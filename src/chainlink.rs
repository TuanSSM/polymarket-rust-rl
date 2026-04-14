use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{Duration, Instant};
use tokio_tungstenite::connect_async;
use tungstenite::http::Request;

use crate::config::ChainlinkConfig;
use crate::error::FeedError;
use crate::signal::OracleTick;
use crate::spsc::SpscProducer;

type HmacSha256 = Hmac<Sha256>;

/// Raw Chainlink Data Streams report.
#[derive(Debug, serde::Deserialize)]
struct ChainlinkMessage {
    report: Option<ChainlinkReport>,
}

#[derive(Debug, serde::Deserialize)]
struct ChainlinkReport {
    #[serde(rename = "feedID")]
    feed_id: String,
    #[serde(rename = "observationsTimestamp")]
    observations_ts: u64,
    #[serde(rename = "benchmarkPrice")]
    benchmark_price: String,
}

pub struct ChainlinkClient {
    config: ChainlinkConfig,
}

impl ChainlinkClient {
    pub fn new(config: ChainlinkConfig) -> Self {
        Self { config }
    }

    /// Compute HMAC-SHA256 signature for auth.
    fn compute_hmac(&self, payload: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(self.config.hmac_secret.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(payload);
        mac.finalize().into_bytes().to_vec()
    }

    /// Build authenticated WS request.
    fn build_request(&self) -> Result<Request<()>, FeedError> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();

        let body = format!("{}{}", self.config.api_key, timestamp);
        let hmac_sig = self.compute_hmac(body.as_bytes());
        let hmac_hex = hex_encode(&hmac_sig);

        let request = Request::builder()
            .uri(&self.config.ws_url)
            .header("Authorization", &self.config.api_key)
            .header("X-Authorization-Timestamp", &timestamp)
            .header("X-Authorization-Signature-SHA256", &hmac_hex)
            .body(())
            .map_err(|e| FeedError::Auth(e.to_string()))?;

        Ok(request)
    }

    /// Parse a report into an OracleTick.
    fn parse_report(&self, report: &ChainlinkReport) -> Option<OracleTick> {
        let price = report.benchmark_price.parse::<f64>().ok()?;
        let feed_id = hash_feed_id(&report.feed_id);

        Some(OracleTick {
            feed_id,
            price,
            oracle_ts_us: report.observations_ts * 1_000_000,
            local_ts: Instant::now(),
        })
    }

    /// Run the WebSocket loop. Reconnects on disconnect with exponential backoff.
    pub async fn run(self, mut tx: SpscProducer<OracleTick>) -> Result<(), FeedError> {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(60);
        let max_retries = 20;
        let mut retries = 0;

        loop {
            match self.connect_and_stream(&mut tx).await {
                Ok(()) => {
                    tracing::info!("chainlink ws stream ended normally");
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
                        backoff_ms = backoff.as_millis(),
                        "chainlink ws disconnected, reconnecting"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                }
            }
        }
    }

    async fn connect_and_stream(
        &self,
        tx: &mut SpscProducer<OracleTick>,
    ) -> Result<(), FeedError> {
        let request = self.build_request()?;
        let (ws_stream, _) = connect_async(request).await?;
        let (mut write, mut read) = ws_stream.split();

        // Subscribe to configured feeds
        let sub_msg = serde_json::json!({
            "type": "subscribe",
            "feedIDs": self.config.feeds,
        });
        write
            .send(tungstenite::Message::Text(sub_msg.to_string()))
            .await?;

        tracing::info!(feeds = ?self.config.feeds, "chainlink ws subscribed");

        while let Some(msg) = read.next().await {
            let msg = msg?;
            if let tungstenite::Message::Text(text) = msg {
                if let Ok(parsed) = serde_json::from_str::<ChainlinkMessage>(&text) {
                    if let Some(report) = parsed.report {
                        if let Some(tick) = self.parse_report(&report) {
                            tx.push_overwrite(tick);
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

fn hash_feed_id(feed_id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    feed_id.hash(&mut hasher);
    hasher.finish()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
