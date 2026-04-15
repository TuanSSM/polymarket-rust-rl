use thiserror::Error;

#[derive(Error, Debug)]
pub enum BotError {
    #[error("feed error on {feed}: {source}")]
    Feed {
        feed: &'static str,
        #[source]
        source: FeedError,
    },
    #[error("engine error: {0}")]
    Engine(#[from] EngineError),
    #[error("clob error: {0}")]
    Clob(#[from] ClobError),
    #[error("config error: {0}")]
    Config(String),
}

#[derive(Error, Debug)]
pub enum FeedError {
    #[error("websocket: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("auth failed: {0}")]
    Auth(String),
    #[error("reconnect limit exceeded")]
    ReconnectExhausted,
}

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("signal stale: last update {ms_ago}ms ago")]
    StaleSignal { ms_ago: u64 },
    #[error("kelly sizing failed: {0}")]
    Kelly(String),
}

#[derive(Error, Debug)]
pub enum ClobError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("signing: {0}")]
    Signing(String),
    #[error("order rejected: {reason}")]
    Rejected { reason: String },
    #[error("rate limited")]
    RateLimited,
}
