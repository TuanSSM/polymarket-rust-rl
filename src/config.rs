use serde::Deserialize;
use std::path::Path;

use crate::error::BotError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    DryRun,
    Live,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub execution: ExecutionConfig,
    pub feeds: FeedConfig,
    pub polymarket: PolymarketConfig,
    pub strategy: StrategyConfig,
    pub risk: RiskConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionConfig {
    pub mode: ExecutionMode,
    #[serde(default = "default_episode_secs")]
    pub episode_secs: u64,
    #[serde(default = "default_max_active_markets")]
    pub max_active_markets: usize,
}

fn default_episode_secs() -> u64 {
    300
}

fn default_max_active_markets() -> usize {
    5
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedConfig {
    pub chainlink: ChainlinkConfig,
    pub binance_spot: ExchangeWsConfig,
    pub binance_futures: ExchangeWsConfig,
    pub bybit: ExchangeWsConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkConfig {
    pub ws_url: String,
    pub api_key: String,
    pub hmac_secret: String,
    pub feeds: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeWsConfig {
    pub ws_url: String,
    pub symbols: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolymarketConfig {
    pub ws_url: String,
    pub rest_url: String,
    pub gamma_url: String,
    pub private_key: String,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyConfig {
    #[serde(default = "default_prior")]
    pub prior_prob: f64,
    pub cvd_weight: f64,
    pub delay_weight: f64,
    pub premium_weight: f64,
    #[serde(default = "default_learning_rate")]
    pub learning_rate: f64,
    #[serde(default = "default_discount")]
    pub discount: f64,
    #[serde(default = "default_epsilon")]
    pub epsilon: f64,
}

fn default_prior() -> f64 {
    0.5
}
fn default_learning_rate() -> f64 {
    0.01
}
fn default_discount() -> f64 {
    0.99
}
fn default_epsilon() -> f64 {
    0.1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskConfig {
    pub max_position_usd: f64,
    pub max_kelly_fraction: f64,
    pub bankroll_usd: f64,
    #[serde(default = "default_min_edge")]
    pub min_edge_bps: f64,
}

fn default_min_edge() -> f64 {
    50.0
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, BotError> {
        let contents =
            std::fs::read_to_string(path).map_err(|e| BotError::Config(e.to_string()))?;
        let config: Config = toml::from_str(&contents).map_err(|e| BotError::Config(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), BotError> {
        if self.execution.mode == ExecutionMode::Live && self.polymarket.private_key.is_empty() {
            return Err(BotError::Config(
                "Live mode requires a non-empty private_key".into(),
            ));
        }
        if self.risk.max_kelly_fraction <= 0.0 || self.risk.max_kelly_fraction > 1.0 {
            return Err(BotError::Config(
                "max_kelly_fraction must be in (0, 1]".into(),
            ));
        }
        if self.risk.bankroll_usd <= 0.0 {
            return Err(BotError::Config("bankroll_usd must be positive".into()));
        }
        if self.strategy.prior_prob <= 0.0 || self.strategy.prior_prob >= 1.0 {
            return Err(BotError::Config(
                "prior_prob must be in (0, 1)".into(),
            ));
        }
        Ok(())
    }
}
