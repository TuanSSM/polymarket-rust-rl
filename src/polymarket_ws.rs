use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_tungstenite::connect_async;

use crate::config::PolymarketConfig;
use crate::error::FeedError;

/// Polymarket order book snapshot.
#[derive(Debug, Clone)]
pub struct OrderBookSnapshot {
    pub market_id: String,
    pub token_id: String,
    pub bids: Vec<(f64, f64)>, // (price, size)
    pub asks: Vec<(f64, f64)>,
    pub ts: Instant,
}

impl OrderBookSnapshot {
    /// Best bid + best ask / 2
    pub fn mid_price(&self) -> Option<f64> {
        let best_bid = self.bids.first().map(|(p, _)| *p)?;
        let best_ask = self.asks.first().map(|(p, _)| *p)?;
        Some((best_bid + best_ask) / 2.0)
    }

    /// Spread in basis points
    pub fn spread_bps(&self) -> Option<f64> {
        let best_bid = self.bids.first().map(|(p, _)| *p)?;
        let best_ask = self.asks.first().map(|(p, _)| *p)?;
        let mid = (best_bid + best_ask) / 2.0;
        if mid <= 0.0 {
            return None;
        }
        Some((best_ask - best_bid) / mid * 10_000.0)
    }

    /// Implied probability from mid price
    pub fn implied_prob(&self) -> Option<f64> {
        self.mid_price()
    }
}

/// Subscription entry: token_id → watch sender for book snapshots.
/// Uses watch channel (latest-value semantics, no Copy requirement).
pub struct MarketSubscription {
    pub token_id: String,
    pub market_id: String,
    pub tx: watch::Sender<Option<OrderBookSnapshot>>,
}

pub struct PolymarketWsClient {
    ws_url: String,
}

impl PolymarketWsClient {
    pub fn new(config: &PolymarketConfig) -> Self {
        Self {
            ws_url: config.ws_url.clone(),
        }
    }

    /// Run the WS loop, subscribing to multiple markets.
    /// Routes book updates to per-market watch senders.
    pub async fn run(self, subscriptions: Vec<MarketSubscription>) -> Result<(), FeedError> {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(60);
        let max_retries = 20;
        let mut retries = 0;

        // Build lookup: token_id → index
        let mut token_map: HashMap<String, usize> = HashMap::new();
        for (i, sub) in subscriptions.iter().enumerate() {
            token_map.insert(sub.token_id.clone(), i);
        }

        loop {
            match self
                .connect_and_stream(&subscriptions, &token_map)
                .await
            {
                Ok(()) => {
                    tracing::info!("polymarket ws stream ended normally");
                    break Ok(());
                }
                Err(e) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(FeedError::ReconnectExhausted);
                    }
                    tracing::warn!(
                        error = %e,
                        retry = retries,
                        "polymarket ws disconnected, reconnecting"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                }
            }
        }
    }

    async fn connect_and_stream(
        &self,
        subscriptions: &[MarketSubscription],
        token_map: &HashMap<String, usize>,
    ) -> Result<(), FeedError> {
        let (ws_stream, _) = connect_async(&self.ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        // Subscribe to each market's book channel
        let asset_ids: Vec<&str> = subscriptions.iter().map(|s| s.token_id.as_str()).collect();

        let sub_msg = serde_json::json!({
            "type": "subscribe",
            "channel": "book",
            "assets_ids": asset_ids,
        });
        write
            .send(tungstenite::Message::Text(sub_msg.to_string()))
            .await?;

        tracing::info!(
            markets = subscriptions.len(),
            "polymarket ws subscribed to book channels"
        );

        while let Some(msg) = read.next().await {
            let msg = msg?;
            if let tungstenite::Message::Text(text) = msg {
                if let Some((token_id, snapshot)) = self.parse_book_update(&text) {
                    if let Some(&idx) = token_map.get(&token_id) {
                        let _ = subscriptions[idx].tx.send(Some(snapshot));
                    }
                }
            }
        }

        Ok(())
    }

    /// Parse a Polymarket WS book update message.
    fn parse_book_update(&self, text: &str) -> Option<(String, OrderBookSnapshot)> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;

        let event_type = v.get("event_type")?.as_str()?;
        if event_type != "book" && event_type != "price_change" {
            return None;
        }

        let market = v.get("market")?;
        let asset_id = v.get("asset_id")?.as_str()?.to_string();

        let bids = Self::parse_levels(market.get("bids")?)?;
        let asks = Self::parse_levels(market.get("asks")?)?;

        let snapshot = OrderBookSnapshot {
            market_id: v
                .get("condition_id")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string(),
            token_id: asset_id.clone(),
            bids,
            asks,
            ts: Instant::now(),
        };

        Some((asset_id, snapshot))
    }

    fn parse_levels(value: &serde_json::Value) -> Option<Vec<(f64, f64)>> {
        let arr = value.as_array()?;
        let mut levels = Vec::with_capacity(arr.len());
        for level in arr {
            let price: f64 = level.get("price")?.as_str()?.parse().ok()?;
            let size: f64 = level.get("size")?.as_str()?.parse().ok()?;
            levels.push((price, size));
        }
        Some(levels)
    }
}
