use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};

use crate::bayesian::BayesianEstimator;
use crate::chainlink::ChainlinkClient;
use crate::config::Config;
use crate::engine::CoreEngine;
use crate::error::BotError;
use crate::exchange_ws::ExchangeWsClient;
use crate::policy::{EpisodeOutcome, LinearPolicy, Params};
use crate::polymarket_rest::{MarketInfo, PolymarketRestClient};
use crate::polymarket_ws::{MarketSubscription, OrderBookSnapshot, PolymarketWsClient};
use crate::seg_lock::{self, SegLockReader, SegLockWriter};
use crate::signal::{Exchange, OracleTick, SignalEngine, SignalSnapshot, TradeEvent};
use crate::spsc;

pub struct Controller {
    config: Config,
    policy: LinearPolicy,
    params_tx: SegLockWriter<Params>,
    params_rx: SegLockReader<Params>,
    rest_client: Arc<PolymarketRestClient>,
}

impl Controller {
    pub fn new(config: Config) -> Result<Self, BotError> {
        let policy = LinearPolicy::new(
            config.strategy.learning_rate,
            config.strategy.discount,
            config.strategy.epsilon,
        );

        let initial_params = policy.export_params(&config.risk);
        let (params_tx, params_rx) = seg_lock::seg_lock(initial_params);

        let rest_client = Arc::new(
            PolymarketRestClient::new(&config.polymarket).map_err(BotError::Clob)?,
        );

        Ok(Self {
            config,
            policy,
            params_tx,
            params_rx,
            rest_client,
        })
    }

    pub async fn run(mut self) -> Result<(), BotError> {
        // 1. Discover markets
        tracing::info!("discovering markets...");
        let markets = self
            .rest_client
            .discover_markets()
            .await
            .map_err(BotError::Clob)?;
        tracing::info!(count = markets.len(), "markets discovered");

        let markets: Vec<MarketInfo> = markets
            .into_iter()
            .filter(|m| !m.tokens.is_empty())
            .take(self.config.execution.max_active_markets)
            .collect();

        if markets.is_empty() {
            tracing::warn!("no active markets found, exiting");
            return Ok(());
        }

        // 2. Create signal broadcast channel
        let (signal_tx, _) = broadcast::channel::<SignalSnapshot>(256);

        // 3. Spawn feed tasks
        self.spawn_feeds(signal_tx.clone());

        // 4. Spawn signal engine
        self.spawn_signal_engine(signal_tx.clone());

        // 5. Spawn CoreEngine per market
        let mut outcome_consumers = Vec::new();
        let mut market_subs = Vec::new();

        for market in &markets {
            let token = match market.tokens.first() {
                Some(t) => t,
                None => continue,
            };

            // Per-market book channel (watch: latest-value semantics)
            let (book_tx, book_rx) =
                watch::channel::<Option<OrderBookSnapshot>>(None);
            market_subs.push(MarketSubscription {
                token_id: token.token_id.clone(),
                market_id: market.condition_id.clone(),
                tx: book_tx,
            });

            // Per-engine outcome SPSC
            let (outcome_tx, outcome_rx) = spsc::spsc_channel(16);
            outcome_consumers.push(outcome_rx);

            let bayesian = BayesianEstimator::new(
                self.config.strategy.cvd_weight,
                self.config.strategy.delay_weight,
                self.config.strategy.premium_weight,
            );

            let engine_policy = LinearPolicy::new(
                self.config.strategy.learning_rate,
                self.config.strategy.discount,
                self.config.strategy.epsilon,
            );

            let engine = CoreEngine::new(
                market.condition_id.clone(),
                token.token_id.clone(),
                signal_tx.subscribe(),
                book_rx,
                self.params_rx.clone(),
                outcome_tx,
                Arc::clone(&self.rest_client),
                self.config.execution.mode,
                self.config.execution.episode_secs,
                0.1, // gate threshold
                bayesian,
                engine_policy,
            );

            tracing::info!(
                market = %market.condition_id,
                question = %market.question,
                "spawning engine"
            );

            tokio::spawn(engine.run());
        }

        // 6. Spawn Polymarket WS for book feeds
        let poly_ws = PolymarketWsClient::new(&self.config.polymarket);
        tokio::spawn(async move {
            if let Err(e) = poly_ws.run(market_subs).await {
                tracing::error!(error = %e, "polymarket ws error");
            }
        });

        // 7. Main control loop
        tracing::info!(
            mode = ?self.config.execution.mode,
            markets = markets.len(),
            "controller running"
        );

        let mut interval = tokio::time::interval(Duration::from_millis(100));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Drain episode outcomes and update RL weights
                    for rx in &mut outcome_consumers {
                        while let Some(outcome) = rx.drain_last() {
                            self.on_episode_outcome(outcome);
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("received ctrl-c, shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    fn on_episode_outcome(&mut self, outcome: EpisodeOutcome) {
        tracing::info!(
            market_id = outcome.market_id,
            pnl = outcome.pnl_usd,
            fills = outcome.fills,
            cancels = outcome.cancels,
            "RL update from episode"
        );

        // TD(0) terminal update on master policy
        self.policy.td0_terminal(
            &outcome.final_state,
            outcome.last_action,
            outcome.pnl_usd,
        );

        // Republish updated weights
        let new_params = self.policy.export_params(&self.config.risk);
        self.params_tx.write(new_params);
    }

    fn spawn_feeds(&self, _signal_tx: broadcast::Sender<SignalSnapshot>) {
        // Chainlink WS
        let chainlink_config = self.config.feeds.chainlink.clone();
        let (cl_tx, _cl_rx) = spsc::spsc_channel::<OracleTick>(256);
        let cl_client = ChainlinkClient::new(chainlink_config);
        tokio::spawn(async move {
            if let Err(e) = cl_client.run(cl_tx).await {
                tracing::error!(error = %e, "chainlink feed error");
            }
        });

        // Binance Spot WS
        let binance_spot_config = self.config.feeds.binance_spot.clone();
        let (bs_tx, _bs_rx) = spsc::spsc_channel::<TradeEvent>(4096);
        let bs_client = ExchangeWsClient::new(Exchange::BinanceSpot, &binance_spot_config);
        tokio::spawn(async move {
            if let Err(e) = bs_client.run(bs_tx).await {
                tracing::error!(error = %e, "binance spot feed error");
            }
        });

        // Binance Futures WS
        let binance_futures_config = self.config.feeds.binance_futures.clone();
        let (bf_tx, _bf_rx) = spsc::spsc_channel::<TradeEvent>(4096);
        let bf_client = ExchangeWsClient::new(Exchange::BinanceFutures, &binance_futures_config);
        tokio::spawn(async move {
            if let Err(e) = bf_client.run(bf_tx).await {
                tracing::error!(error = %e, "binance futures feed error");
            }
        });

        // Bybit WS
        let bybit_config = self.config.feeds.bybit.clone();
        let (bb_tx, _bb_rx) = spsc::spsc_channel::<TradeEvent>(4096);
        let bb_client = ExchangeWsClient::new(Exchange::Bybit, &bybit_config);
        tokio::spawn(async move {
            if let Err(e) = bb_client.run(bb_tx).await {
                tracing::error!(error = %e, "bybit feed error");
            }
        });
    }

    fn spawn_signal_engine(&self, signal_tx: broadcast::Sender<SignalSnapshot>) {
        tokio::spawn(async move {
            let engine = SignalEngine::new(Duration::from_secs(60));
            let mut interval = tokio::time::interval(Duration::from_millis(50));

            loop {
                interval.tick().await;
                let snapshot = engine.snapshot();
                if signal_tx.send(snapshot).is_err() {
                    // No receivers yet, keep running
                }
            }
        });
    }
}
