mod bayesian;
mod chainlink;
mod config;
mod controller;
mod engine;
mod error;
mod exchange_ws;
mod gate;
mod kelly;
mod policy;
mod polymarket_rest;
mod polymarket_ws;
mod seg_lock;
mod signal;
mod spsc;

use std::path::Path;

use config::Config;
use controller::Controller;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config_path = std::env::args().nth(1).unwrap_or_else(|| "config.toml".into());
    let config = Config::load(Path::new(&config_path))?;

    tracing::info!(mode = ?config.execution.mode, "starting polymarket bot");

    let controller = Controller::new(config)?;
    controller.run().await?;

    Ok(())
}
