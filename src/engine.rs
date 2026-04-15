use rand::rngs::SmallRng;
use rand::SeedableRng;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};

use crate::bayesian::BayesianEstimator;
use crate::config::ExecutionMode;
use crate::error::EngineError;
use crate::gate::BranchlessGate;
use crate::kelly::KellySizer;
use crate::policy::{Action, EpisodeOutcome, LinearPolicy, Params, StateVec};
use crate::polymarket_rest::{ClobOrder, OrderSide, PolymarketRestClient};
use crate::polymarket_ws::OrderBookSnapshot;
use crate::seg_lock::SegLockReader;
use crate::signal::SignalSnapshot;
use crate::spsc::SpscProducer;

/// CoreEngine: runs the hot-path pipeline for one active market.
pub struct CoreEngine {
    market_id: String,
    token_id: String,

    // Inputs
    signal_rx: broadcast::Receiver<SignalSnapshot>,
    book_rx: watch::Receiver<Option<OrderBookSnapshot>>,
    params_rx: SegLockReader<Params>,

    // Output
    outcome_tx: SpscProducer<EpisodeOutcome>,

    // Strategy components (owned, no sharing)
    gate: BranchlessGate,
    bayesian: BayesianEstimator,
    kelly: KellySizer,
    policy: LinearPolicy,

    // Execution
    rest_client: Arc<PolymarketRestClient>,
    mode: ExecutionMode,

    // Episode state
    episode_duration: Duration,
    rng: SmallRng,
}

impl CoreEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        market_id: String,
        token_id: String,
        signal_rx: broadcast::Receiver<SignalSnapshot>,
        book_rx: watch::Receiver<Option<OrderBookSnapshot>>,
        params_rx: SegLockReader<Params>,
        outcome_tx: SpscProducer<EpisodeOutcome>,
        rest_client: Arc<PolymarketRestClient>,
        mode: ExecutionMode,
        episode_secs: u64,
        gate_threshold: f64,
        bayesian: BayesianEstimator,
        policy: LinearPolicy,
    ) -> Self {
        let params = params_rx.read();
        let kelly = KellySizer::new(
            params.kelly_fraction_cap,
            params.bankroll,
            params.min_edge_bps,
        );

        Self {
            market_id,
            token_id,
            signal_rx,
            book_rx,
            params_rx,
            outcome_tx,
            gate: BranchlessGate::new(gate_threshold),
            bayesian,
            kelly,
            policy,
            rest_client,
            mode,
            episode_duration: Duration::from_secs(episode_secs),
            rng: SmallRng::from_entropy(),
        }
    }

    /// Run continuous episodes until cancelled.
    pub async fn run(mut self) {
        loop {
            match self.run_episode().await {
                Ok(outcome) => {
                    tracing::info!(
                        market = %self.market_id,
                        pnl = outcome.pnl_usd,
                        fills = outcome.fills,
                        cancels = outcome.cancels,
                        duration_ms = outcome.duration_ms,
                        "episode complete"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        market = %self.market_id,
                        error = %e,
                        "episode error"
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Main loop: runs for one episode, then sends outcome.
    async fn run_episode(&mut self) -> Result<EpisodeOutcome, EngineError> {
        let episode_start = Instant::now();
        let deadline = episode_start + self.episode_duration;

        let mut position_usd: f64 = 0.0;
        let mut pnl_usd: f64 = 0.0;
        let mut fills: u32 = 0;
        let mut cancels: u32 = 0;
        let mut prev_state: Option<StateVec> = None;
        let mut prev_action: Option<Action> = None;
        let mut prev_pnl: f64 = 0.0;

        loop {
            // 1. Read latest params (seqlock, lock-free)
            let params = self.params_rx.read();
            self.policy.load_params(&params);
            self.kelly = KellySizer::new(
                params.kelly_fraction_cap,
                params.bankroll,
                params.min_edge_bps,
            );

            // 2. Receive signal snapshot (broadcast)
            let signal = match tokio::time::timeout(
                Duration::from_millis(100),
                self.signal_rx.recv(),
            )
            .await
            {
                Ok(Ok(s)) => s,
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    tracing::debug!(lagged = n, "signal receiver lagged");
                    continue;
                }
                Ok(Err(_)) => break, // channel closed
                Err(_) => continue,  // timeout, retry
            };

            // 3. Read order book (watch channel, latest value)
            let book = match self.book_rx.borrow().clone() {
                Some(b) => b,
                None => continue,
            };

            // 4. Gate: zero out weak signals
            let gated_cvd = self.gate.apply(signal.spot_cvd);
            let gated_perp_cvd = self.gate.apply(signal.perp_cvd);
            let gated_signal = SignalSnapshot {
                spot_cvd: gated_cvd,
                perp_cvd: gated_perp_cvd,
                ..signal
            };

            // 5. Bayesian estimate
            let market_prob = book.implied_prob().unwrap_or(0.5);
            let estimate = self.bayesian.estimate(0.5, &gated_signal, market_prob);

            // 6. Kelly sizing
            let size = self.kelly.size(estimate.prob_yes, market_prob, estimate.direction);

            // 7. Build state vector
            let elapsed_frac = episode_start.elapsed().as_secs_f64()
                / self.episode_duration.as_secs_f64();
            let state = StateVec {
                edge_bps: estimate.edge_bps / 100.0,
                cvd_norm: gated_signal.spot_cvd.tanh(),
                delay_norm: (gated_signal.oracle_delay_ms / 500.0).tanh(),
                premium_norm: gated_signal.spot_perp_premium.tanh(),
                time_in_episode_frac: elapsed_frac,
                position_frac: if params.max_position_usd > 0.0 {
                    position_usd / params.max_position_usd
                } else {
                    0.0
                },
                spread_bps: book.spread_bps().unwrap_or(100.0) / 100.0,
            };

            // 8. RL action selection
            let action = self.policy.select_action(&state, &mut self.rng);

            // 9. Execute action
            match self.mode {
                ExecutionMode::DryRun => {
                    match action {
                        Action::PostBid | Action::MarketBuy => {
                            if size.stake_usd > 0.0 {
                                position_usd += size.stake_usd;
                                fills += 1;
                                pnl_usd += size.stake_usd * estimate.edge_bps.abs()
                                    / 10_000.0
                                    / 2.0;
                            }
                        }
                        Action::PostAsk | Action::MarketSell => {
                            if position_usd > 0.0 {
                                position_usd = (position_usd - size.stake_usd).max(0.0);
                                fills += 1;
                            }
                        }
                        Action::CancelAll => {
                            cancels += 1;
                        }
                        Action::Hold => {}
                    }

                    tracing::debug!(
                        action = ?action,
                        edge_bps = estimate.edge_bps,
                        stake = size.stake_usd,
                        position = position_usd,
                        pnl = pnl_usd,
                        market = %self.market_id,
                        "[DRY RUN]"
                    );
                }
                ExecutionMode::Live => {
                    if let Err(e) = self
                        .execute_live(action, &estimate, &size, &book, &mut position_usd, &mut fills, &mut cancels)
                        .await
                    {
                        tracing::error!(error = %e, "live execution error");
                    }
                }
            }

            // 10. TD(0) update with step reward
            if let (Some(prev_s), Some(prev_a)) = (prev_state, prev_action) {
                let pnl_delta = pnl_usd - prev_pnl;
                let reward = pnl_delta
                    + 0.0001 * fills as f64
                    - 0.00005 * cancels as f64
                    - 0.0001 * position_usd.abs();
                self.policy.td0_update(&prev_s, prev_a, reward, &state);
            }
            prev_state = Some(state);
            prev_action = Some(action);
            prev_pnl = pnl_usd;

            // 11. Check episode end
            if Instant::now() >= deadline {
                break;
            }
        }

        // Terminal TD(0) update
        if let (Some(prev_s), Some(prev_a)) = (prev_state, prev_action) {
            let terminal_reward = pnl_usd - 0.001 * position_usd.abs();
            self.policy.td0_terminal(&prev_s, prev_a, terminal_reward);
        }

        let market_id_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.market_id.hash(&mut h);
            h.finish()
        };

        let outcome = EpisodeOutcome {
            market_id: market_id_hash,
            pnl_usd,
            fills,
            cancels,
            final_state: prev_state.unwrap_or_default(),
            last_action: prev_action.unwrap_or(Action::Hold),
            duration_ms: episode_start.elapsed().as_millis() as u64,
        };

        let _ = self.outcome_tx.try_push(outcome);
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_live(
        &self,
        action: Action,
        estimate: &crate::bayesian::ProbEstimate,
        size: &crate::kelly::SizeDecision,
        book: &OrderBookSnapshot,
        position_usd: &mut f64,
        fills: &mut u32,
        cancels: &mut u32,
    ) -> Result<(), crate::error::ClobError> {
        match action {
            Action::Hold => {}
            Action::PostBid => {
                if size.stake_usd > 0.0 {
                    if let Some(bid_price) = book.bids.first().map(|(p, _)| *p) {
                        let order = ClobOrder {
                            token_id: self.token_id.clone(),
                            side: OrderSide::Buy,
                            price: bid_price,
                            size: size.stake_usd,
                        };
                        let resp = self.rest_client.place_order(&order).await?;
                        tracing::info!(order_id = %resp.order_id, price = bid_price, "posted bid");
                        *position_usd += size.stake_usd;
                        *fills += 1;
                    }
                }
            }
            Action::PostAsk => {
                if let Some(ask_price) = book.asks.first().map(|(p, _)| *p) {
                    let order = ClobOrder {
                        token_id: self.token_id.clone(),
                        side: OrderSide::Sell,
                        price: ask_price,
                        size: position_usd.abs().min(size.stake_usd),
                    };
                    let resp = self.rest_client.place_order(&order).await?;
                    tracing::info!(order_id = %resp.order_id, price = ask_price, "posted ask");
                    *position_usd = (*position_usd - size.stake_usd).max(0.0);
                    *fills += 1;
                }
            }
            Action::CancelAll => {
                self.rest_client.cancel_all().await?;
                *cancels += 1;
                tracing::info!("cancelled all orders");
            }
            Action::MarketBuy => {
                if size.stake_usd > 0.0 {
                    if let Some(ask_price) = book.asks.first().map(|(p, _)| *p) {
                        let order = ClobOrder {
                            token_id: self.token_id.clone(),
                            side: OrderSide::Buy,
                            price: ask_price,
                            size: size.stake_usd,
                        };
                        let resp = self.rest_client.place_order(&order).await?;
                        tracing::info!(
                            order_id = %resp.order_id,
                            edge_bps = estimate.edge_bps,
                            "market buy"
                        );
                        *position_usd += size.stake_usd;
                        *fills += 1;
                    }
                }
            }
            Action::MarketSell => {
                if *position_usd > 0.0 {
                    if let Some(bid_price) = book.bids.first().map(|(p, _)| *p) {
                        let order = ClobOrder {
                            token_id: self.token_id.clone(),
                            side: OrderSide::Sell,
                            price: bid_price,
                            size: *position_usd,
                        };
                        let resp = self.rest_client.place_order(&order).await?;
                        tracing::info!(
                            order_id = %resp.order_id,
                            edge_bps = estimate.edge_bps,
                            "market sell"
                        );
                        *position_usd = 0.0;
                        *fills += 1;
                    }
                }
            }
        }
        Ok(())
    }
}
