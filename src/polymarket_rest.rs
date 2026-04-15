use serde::Deserialize;

use crate::config::PolymarketConfig;
use crate::error::ClobError;

/// Market info from Gamma API.
#[derive(Debug, Clone, Deserialize)]
pub struct MarketInfo {
    #[serde(rename = "conditionId", alias = "condition_id")]
    pub condition_id: String,
    pub question: String,
    pub tokens: Vec<TokenInfo>,
    #[serde(rename = "endDate", alias = "end_date_iso", default)]
    pub end_date: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub closed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenInfo {
    pub token_id: String,
    pub outcome: String,
    #[serde(default)]
    pub price: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// CLOB order to place.
#[derive(Debug, Clone)]
pub struct ClobOrder {
    pub token_id: String,
    pub side: OrderSide,
    pub price: f64,
    pub size: f64,
}

/// Response from order placement.
#[derive(Debug, Clone, Deserialize)]
pub struct OrderResponse {
    #[serde(rename = "orderID", alias = "order_id", default)]
    pub order_id: String,
    #[serde(default)]
    pub status: String,
}

pub struct PolymarketRestClient {
    http: reqwest::Client,
    rest_url: String,
    gamma_url: String,
    api_key: String,
    api_secret: String,
    passphrase: String,
    _private_key: String,
}

impl PolymarketRestClient {
    pub fn new(config: &PolymarketConfig) -> Result<Self, ClobError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            http,
            rest_url: config.rest_url.clone(),
            gamma_url: config.gamma_url.clone(),
            api_key: config.api_key.clone(),
            api_secret: config.api_secret.clone(),
            passphrase: config.passphrase.clone(),
            _private_key: config.private_key.clone(),
        })
    }

    /// Discover active markets from Gamma API.
    pub async fn discover_markets(&self) -> Result<Vec<MarketInfo>, ClobError> {
        let url = format!("{}/markets", self.gamma_url);
        let resp = self
            .http
            .get(&url)
            .query(&[("active", "true"), ("closed", "false")])
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ClobError::RateLimited);
        }

        let markets: Vec<MarketInfo> = resp.json().await?;
        Ok(markets
            .into_iter()
            .filter(|m| m.active && !m.closed)
            .collect())
    }

    /// Build auth headers for CLOB API.
    fn auth_headers(&self) -> Vec<(&'static str, String)> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();

        // HMAC signature: HMAC-SHA256(secret, timestamp + method + path + body)
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let msg = format!("{timestamp}GET/order");
        let mut mac = Hmac::<Sha256>::new_from_slice(self.api_secret.as_bytes())
            .expect("HMAC key length");
        mac.update(msg.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = base64_encode(&sig);

        vec![
            ("POLY_ADDRESS", self.api_key.clone()),
            ("POLY_SIGNATURE", sig_b64),
            ("POLY_TIMESTAMP", timestamp),
            ("POLY_PASSPHRASE", self.passphrase.clone()),
        ]
    }

    /// Place a CLOB order.
    /// In production this requires EIP-712 signing of the order struct.
    pub async fn place_order(&self, order: &ClobOrder) -> Result<OrderResponse, ClobError> {
        let url = format!("{}/order", self.rest_url);

        let side_str = match order.side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let body = serde_json::json!({
            "tokenID": order.token_id,
            "price": format!("{:.2}", order.price),
            "size": format!("{:.2}", order.size),
            "side": side_str,
            "type": "GTC",
        });

        let mut req = self.http.post(&url).json(&body);
        for (key, val) in self.auth_headers() {
            req = req.header(key, val);
        }

        let resp = req.send().await?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ClobError::RateLimited);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClobError::Rejected {
                reason: format!("{status}: {text}"),
            });
        }

        let order_resp: OrderResponse = resp.json().await?;
        Ok(order_resp)
    }

    /// Cancel an order by ID.
    pub async fn cancel_order(&self, order_id: &str) -> Result<(), ClobError> {
        let url = format!("{}/order/{}", self.rest_url, order_id);

        let mut req = self.http.delete(&url);
        for (key, val) in self.auth_headers() {
            req = req.header(key, val);
        }

        let resp = req.send().await?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ClobError::RateLimited);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClobError::Rejected {
                reason: format!("{status}: {text}"),
            });
        }

        Ok(())
    }

    /// Cancel all open orders.
    pub async fn cancel_all(&self) -> Result<(), ClobError> {
        let url = format!("{}/cancel-all", self.rest_url);

        let mut req = self.http.delete(&url);
        for (key, val) in self.auth_headers() {
            req = req.header(key, val);
        }

        let resp = req.send().await?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ClobError::RateLimited);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClobError::Rejected {
                reason: format!("{status}: {text}"),
            });
        }

        Ok(())
    }
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}
