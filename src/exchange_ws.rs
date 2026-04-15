use futures_util::{SinkExt, StreamExt};
use std::time::{Duration, Instant};
use tokio_tungstenite::connect_async;

use crate::config::ExchangeWsConfig;
use crate::error::FeedError;
use crate::signal::{Exchange, TradeEvent};
use crate::spsc::SpscProducer;

/// Unified WS client for Binance Spot, Binance Futures, and Bybit.
pub struct ExchangeWsClient {
    exchange: Exchange,
    ws_url: String,
    symbols: Vec<String>,
}

impl ExchangeWsClient {
    pub fn new(exchange: Exchange, config: &ExchangeWsConfig) -> Self {
        Self {
            exchange,
            ws_url: config.ws_url.clone(),
            symbols: config.symbols.clone(),
        }
    }

    /// Build the subscription message per exchange protocol.
    fn subscribe_msg(&self) -> String {
        match self.exchange {
            Exchange::BinanceSpot | Exchange::BinanceFutures => {
                let streams: Vec<String> = self
                    .symbols
                    .iter()
                    .map(|s| format!("{}@trade", s.to_lowercase()))
                    .collect();
                serde_json::json!({
                    "method": "SUBSCRIBE",
                    "params": streams,
                    "id": 1
                })
                .to_string()
            }
            Exchange::Bybit => {
                let args: Vec<String> = self
                    .symbols
                    .iter()
                    .map(|s| format!("publicTrade.{s}"))
                    .collect();
                serde_json::json!({
                    "op": "subscribe",
                    "args": args
                })
                .to_string()
            }
        }
    }

    /// Parse exchange-specific JSON into TradeEvent.
    fn parse_trade(&self, text: &str) -> Option<TradeEvent> {
        match self.exchange {
            Exchange::BinanceSpot | Exchange::BinanceFutures => {
                self.parse_binance_trade(text)
            }
            Exchange::Bybit => self.parse_bybit_trade(text),
        }
    }

    fn parse_binance_trade(&self, text: &str) -> Option<TradeEvent> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;

        // Binance trade stream format: {"e":"trade","s":"BTCUSDT","p":"...","q":"...","m":true,...}
        if v.get("e")?.as_str()? != "trade" {
            return None;
        }

        let symbol = v.get("s")?.as_str()?;
        let price: f64 = v.get("p")?.as_str()?.parse().ok()?;
        let qty: f64 = v.get("q")?.as_str()?.parse().ok()?;
        let is_buyer_maker = v.get("m")?.as_bool()?;
        let exchange_ts = v.get("T")?.as_u64()?;

        Some(TradeEvent {
            exchange: self.exchange,
            symbol_hash: hash_symbol(symbol),
            price,
            qty,
            is_buy: !is_buyer_maker, // taker side: if buyer is maker, taker is seller
            local_ts: Instant::now(),
            exchange_ts_us: exchange_ts * 1000, // ms → us
        })
    }

    fn parse_bybit_trade(&self, text: &str) -> Option<TradeEvent> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;

        // Bybit format: {"topic":"publicTrade.BTCUSDT","data":[{"s":"BTCUSDT","p":"...","v":"...","S":"Buy",...}]}
        if v.get("topic")?.as_str()?.starts_with("publicTrade.") {
            let data = v.get("data")?.as_array()?;
            let trade = data.last()?; // take most recent if batched

            let symbol = trade.get("s")?.as_str()?;
            let price: f64 = trade.get("p")?.as_str()?.parse().ok()?;
            let qty: f64 = trade.get("v")?.as_str()?.parse().ok()?;
            let side = trade.get("S")?.as_str()?;
            let ts = trade.get("T")?.as_u64()?;

            return Some(TradeEvent {
                exchange: self.exchange,
                symbol_hash: hash_symbol(symbol),
                price,
                qty,
                is_buy: side == "Buy",
                local_ts: Instant::now(),
                exchange_ts_us: ts * 1000,
            });
        }

        None
    }

    /// Run the WS loop. Pushes TradeEvent to SPSC.
    /// Reconnects on disconnect with exponential backoff.
    pub async fn run(self, mut tx: SpscProducer<TradeEvent>) -> Result<(), FeedError> {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(60);
        let max_retries = 20;
        let mut retries = 0;

        loop {
            match self.connect_and_stream(&mut tx).await {
                Ok(()) => {
                    tracing::info!(exchange = ?self.exchange, "ws stream ended normally");
                    break Ok(());
                }
                Err(e) => {
                    retries += 1;
                    if retries > max_retries {
                        return Err(FeedError::ReconnectExhausted);
                    }
                    tracing::warn!(
                        exchange = ?self.exchange,
                        error = %e,
                        retry = retries,
                        "ws disconnected, reconnecting"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                }
            }
        }
    }

    async fn connect_and_stream(
        &self,
        tx: &mut SpscProducer<TradeEvent>,
    ) -> Result<(), FeedError> {
        let (ws_stream, _) = connect_async(&self.ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        // Subscribe
        let sub = self.subscribe_msg();
        write.send(tungstenite::Message::Text(sub)).await?;

        tracing::info!(
            exchange = ?self.exchange,
            symbols = ?self.symbols,
            "exchange ws subscribed"
        );

        while let Some(msg) = read.next().await {
            let msg = msg?;
            match msg {
                tungstenite::Message::Text(text) => {
                    if let Some(trade) = self.parse_trade(&text) {
                        tx.push_overwrite(trade);
                    }
                }
                tungstenite::Message::Ping(payload) => {
                    write.send(tungstenite::Message::Pong(payload)).await?;
                }
                _ => {}
            }
        }

        Ok(())
    }
}

fn hash_symbol(symbol: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    symbol.hash(&mut hasher);
    hasher.finish()
}
