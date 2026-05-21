use coin_listener_core::models::AddressEvent;
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;
use std::{collections::BTreeMap, time::Duration};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalChannelType {
    Telegram,
    Webhook,
}

impl ExternalChannelType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Webhook => "webhook",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalConfigError {
    pub message: String,
}

impl ExternalConfigError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramChannelConfig {
    pub telegram_bot_id: Option<Uuid>,
    pub bot_token_env: Option<String>,
    pub chat_id: String,
}

impl TelegramChannelConfig {
    pub fn parse(value: &Value) -> Result<Self, ExternalConfigError> {
        let telegram_bot_id = optional_uuid(value, "telegram_bot_id")?;
        let bot_token_env = optional_string(value, "bot_token_env");
        if telegram_bot_id.is_none() && bot_token_env.is_none() {
            return Err(ExternalConfigError::new(
                "telegram telegram_bot_id or bot_token_env is required",
            ));
        }
        let chat_id = required_string(value, "chat_id", "telegram chat_id is required")?;
        Ok(Self {
            telegram_bot_id,
            bot_token_env,
            chat_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookChannelConfig {
    pub url: String,
    pub secret_env: Option<String>,
    pub timeout_ms: u64,
}

impl WebhookChannelConfig {
    pub fn parse(value: &Value) -> Result<Self, ExternalConfigError> {
        let url = required_string(value, "url", "webhook url is required")?;
        let parsed_url = reqwest::Url::parse(&url)
            .map_err(|_| ExternalConfigError::new("webhook url must use http or https"))?;
        let lower_url = url.to_ascii_lowercase();
        if !matches!(parsed_url.scheme(), "http" | "https")
            || parsed_url.host_str().is_none()
            || lower_url.starts_with("http:///")
            || lower_url.starts_with("https:///")
        {
            return Err(ExternalConfigError::new(
                "webhook url must use http or https",
            ));
        }
        let secret_env = optional_string(value, "secret_env");
        let timeout_ms = value
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(5000)
            .clamp(1000, 30000);
        Ok(Self {
            url,
            secret_env,
            timeout_ms,
        })
    }
}

pub fn notification_idempotency_key(
    tenant_id: Uuid,
    event_id: Uuid,
    rule_id: Uuid,
    channel_id: Uuid,
) -> String {
    format!("notification:v1:{tenant_id}:{event_id}:{rule_id}:{channel_id}")
}

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct ExternalNotificationSender {
    client: Client,
    telegram_api_base_url: String,
}

impl ExternalNotificationSender {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            telegram_api_base_url: "https://api.telegram.org".to_string(),
        }
    }

    #[cfg(test)]
    fn with_telegram_api_base_url(client: Client, telegram_api_base_url: String) -> Self {
        Self {
            client,
            telegram_api_base_url,
        }
    }

    pub async fn send_telegram(
        &self,
        config: &TelegramChannelConfig,
        bot_token: &str,
        text: &str,
    ) -> ExternalSendOutcome {
        let url = self.telegram_bot_method_url(bot_token, "sendMessage");
        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_millis(5000))
            .json(&serde_json::json!({
                "chat_id": config.chat_id,
                "text": text,
            }))
            .send()
            .await;

        match response {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = read_provider_response_prefix(response).await;
                classify_telegram_response(status, &body)
            }
            Err(error) => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
                last_error: Some(telegram_network_error_message(&url, &error.to_string())),
                provider_message_id: None,
                provider_status_code: None,
                provider_response: None,
            }),
        }
    }

    pub async fn verify_telegram_bot(&self, bot_token: &str) -> ExternalSendOutcome {
        let url = self.telegram_bot_method_url(bot_token, "getMe");
        let response = self
            .client
            .get(&url)
            .timeout(Duration::from_millis(5000))
            .send()
            .await;

        match response {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = read_provider_response_prefix(response).await;
                classify_telegram_verify_response(status, &body)
            }
            Err(error) => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
                last_error: Some(telegram_network_error_message(&url, &error.to_string())),
                provider_message_id: None,
                provider_status_code: None,
                provider_response: None,
            }),
        }
    }

    fn telegram_bot_method_url(&self, bot_token: &str, method: &str) -> String {
        format!(
            "{}/bot{}/{}",
            self.telegram_api_base_url.trim_end_matches('/'),
            bot_token,
            method
        )
    }

    pub async fn send_webhook(&self, parts: WebhookRequestParts) -> ExternalSendOutcome {
        let mut request = self
            .client
            .post(&parts.url)
            .timeout(parts.timeout)
            .body(parts.body);
        for (name, value) in parts.headers {
            request = request.header(name, value);
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = read_provider_response_prefix(response).await;
                classify_webhook_response(status, &body)
            }
            Err(error) => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
                last_error: Some(webhook_network_error_message(
                    &parts.url,
                    &error.to_string(),
                )),
                provider_message_id: None,
                provider_status_code: None,
                provider_response: None,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSendMetadata {
    pub last_error: Option<String>,
    pub provider_message_id: Option<String>,
    pub provider_status_code: Option<i32>,
    pub provider_response: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalSendOutcome {
    Sent(ExternalSendMetadata),
    PermanentFailure(ExternalSendMetadata),
    TransientFailure(ExternalSendMetadata),
}

impl ExternalSendOutcome {
    pub fn is_sent(&self) -> bool {
        matches!(self, Self::Sent(_))
    }

    pub fn is_permanent_failure(&self) -> bool {
        matches!(self, Self::PermanentFailure(_))
    }

    pub fn is_transient_failure(&self) -> bool {
        matches!(self, Self::TransientFailure(_))
    }

    pub fn metadata(&self) -> &ExternalSendMetadata {
        match self {
            Self::Sent(metadata)
            | Self::PermanentFailure(metadata)
            | Self::TransientFailure(metadata) => metadata,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookRequestParts {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebhookPayload {
    pub idempotency_key: String,
    pub event_id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address_id: Uuid,
    pub asset_id: Uuid,
    pub event_type: String,
    pub direction: String,
    pub is_transfer: bool,
    pub tx_hash: Option<String>,
    pub block_number: Option<i64>,
    pub from_address: Option<String>,
    pub to_address: Option<String>,
    pub amount_raw: Option<String>,
    pub amount_decimal: Option<String>,
    pub detected_at: String,
}

pub fn build_webhook_payload(event: &AddressEvent, idempotency_key: &str) -> WebhookPayload {
    WebhookPayload {
        idempotency_key: idempotency_key.to_string(),
        event_id: event.id,
        tenant_id: event.tenant_id,
        chain_id: event.chain_id,
        address_id: event.address_id,
        asset_id: event.asset_id,
        event_type: event.event_type.clone(),
        direction: event.direction.clone(),
        is_transfer: event.is_transfer,
        tx_hash: event.tx_hash.clone(),
        block_number: event.block_number,
        from_address: event.from_address.clone(),
        to_address: event.to_address.clone(),
        amount_raw: event.amount_raw.clone(),
        amount_decimal: event.amount_decimal.clone(),
        detected_at: event.detected_at.to_rfc3339(),
    }
}

pub fn build_webhook_request_parts(
    config: &WebhookChannelConfig,
    event: &AddressEvent,
    idempotency_key: &str,
    secret: Option<&str>,
) -> Result<WebhookRequestParts, ExternalConfigError> {
    let payload = build_webhook_payload(event, idempotency_key);
    let body = serde_json::to_string(&payload).map_err(|error| {
        ExternalConfigError::new(format!("webhook payload serialization failed: {error}"))
    })?;
    let mut headers = BTreeMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("X-Coin-Listener-Event-Id".to_string(), event.id.to_string());
    headers.insert(
        "X-Coin-Listener-Idempotency-Key".to_string(),
        idempotency_key.to_string(),
    );
    if let Some(secret) = secret {
        headers.insert(
            "X-Coin-Listener-Signature".to_string(),
            webhook_signature(secret, body.as_bytes()),
        );
    }
    Ok(WebhookRequestParts {
        url: config.url.clone(),
        headers,
        body,
        timeout: Duration::from_millis(config.timeout_ms),
    })
}

pub fn webhook_signature(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any size");
    mac.update(body);
    bytes_to_lower_hex(&mac.finalize().into_bytes())
}

pub fn bytes_to_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn classify_webhook_response(status_code: u16, body: &str) -> ExternalSendOutcome {
    let metadata = ExternalSendMetadata {
        last_error: None,
        provider_message_id: None,
        provider_status_code: Some(status_code as i32),
        provider_response: Some(truncate_provider_response(body)),
    };
    match status_code {
        200..=299 => ExternalSendOutcome::Sent(metadata),
        408 | 429 | 500..=599 => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
            last_error: Some(format!("webhook returned retryable status {status_code}")),
            ..metadata
        }),
        _ => ExternalSendOutcome::PermanentFailure(ExternalSendMetadata {
            last_error: Some(format!("webhook returned permanent status {status_code}")),
            ..metadata
        }),
    }
}

pub fn classify_telegram_verify_response(status_code: u16, body: &str) -> ExternalSendOutcome {
    let parsed_body = serde_json::from_str::<Value>(body).ok();
    let telegram_success = parsed_body
        .as_ref()
        .and_then(|value| value.get("ok"))
        .and_then(Value::as_bool)
        == Some(true);

    if matches!(status_code, 200..=299) && telegram_success {
        return ExternalSendOutcome::Sent(ExternalSendMetadata {
            last_error: None,
            provider_message_id: None,
            provider_status_code: Some(status_code as i32),
            provider_response: Some(truncate_provider_response(body)),
        });
    }

    classify_telegram_response(status_code, body)
}

pub fn classify_telegram_response(status_code: u16, body: &str) -> ExternalSendOutcome {
    let parsed_body = serde_json::from_str::<Value>(body).ok();
    let provider_message_id = parsed_body
        .as_ref()
        .and_then(|value| value.get("result"))
        .and_then(|result| result.get("message_id"))
        .and_then(|message_id| message_id.as_i64().map(|value| value.to_string()));
    let telegram_success = parsed_body
        .as_ref()
        .and_then(|value| value.get("ok"))
        .and_then(Value::as_bool)
        == Some(true)
        && provider_message_id.is_some();

    let metadata = ExternalSendMetadata {
        last_error: None,
        provider_message_id,
        provider_status_code: Some(status_code as i32),
        provider_response: Some(truncate_provider_response(body)),
    };
    match status_code {
        200..=299 if telegram_success => ExternalSendOutcome::Sent(metadata),
        200..=299 if parsed_body.is_none() => {
            ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
                last_error: Some("telegram returned unparsable success body".to_string()),
                ..metadata
            })
        }
        200..=299 => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
            last_error: Some("telegram returned unsuccessful success body".to_string()),
            ..metadata
        }),
        408 | 429 | 500..=599 => ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
            last_error: Some(format!("telegram returned retryable status {status_code}")),
            ..metadata
        }),
        _ => ExternalSendOutcome::PermanentFailure(ExternalSendMetadata {
            last_error: Some(format!("telegram returned permanent status {status_code}")),
            ..metadata
        }),
    }
}

pub fn telegram_network_error_message(url: &str, error: &str) -> String {
    format!(
        "telegram {} failed: {}",
        redact_telegram_url(url),
        redact_telegram_urls_in_text(error)
    )
}

pub fn render_external_notification_text(event: &AddressEvent) -> String {
    let amount = event
        .amount_decimal
        .as_deref()
        .or(event.amount_raw.as_deref())
        .unwrap_or("-");
    let tx_hash = event.tx_hash.as_deref().unwrap_or("-");
    format!(
        "{} {}\naddress: {}\nasset: {}\namount: {}\ntx: {}",
        event.event_type, event.direction, event.address_id, event.asset_id, amount, tx_hash
    )
}

pub fn webhook_network_error_message(url: &str, error: &str) -> String {
    format!(
        "webhook {} failed: {}",
        redact_webhook_url(url),
        redact_webhook_urls_in_text(error)
    )
}

pub fn redact_webhook_urls_in_text(text: &str) -> String {
    redact_urls_in_text(text, redact_webhook_url)
}

pub fn redact_telegram_urls_in_text(text: &str) -> String {
    redact_urls_in_text(text, redact_telegram_url)
}

fn redact_urls_in_text(text: &str, redact_url: fn(&str) -> String) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(url_start) = find_url_start(rest) {
        output.push_str(&rest[..url_start]);
        let after_prefix = &rest[url_start..];
        let url_end = after_prefix
            .find(char::is_whitespace)
            .unwrap_or(after_prefix.len());
        let (url, tail) = after_prefix.split_at(url_end);
        output.push_str(&redact_url(url));
        rest = tail;
    }
    output.push_str(rest);
    output
}

fn find_url_start(text: &str) -> Option<usize> {
    let lower_text = text.to_ascii_lowercase();
    match (lower_text.find("http://"), lower_text.find("https://")) {
        (Some(http), Some(https)) => Some(http.min(https)),
        (Some(http), None) => Some(http),
        (None, Some(https)) => Some(https),
        (None, None) => None,
    }
}

pub const PROVIDER_RESPONSE_MAX_BYTES: usize = 2048;

pub async fn read_provider_response_prefix(mut response: reqwest::Response) -> String {
    let mut bytes = Vec::with_capacity(PROVIDER_RESPONSE_MAX_BYTES);
    while bytes.len() < PROVIDER_RESPONSE_MAX_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = PROVIDER_RESPONSE_MAX_BYTES - bytes.len();
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn truncate_provider_response(body: &str) -> String {
    if body.len() <= PROVIDER_RESPONSE_MAX_BYTES {
        return body.to_string();
    }
    let mut end = PROVIDER_RESPONSE_MAX_BYTES;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    body[..end].to_string()
}

pub fn redact_telegram_url(url: &str) -> String {
    let Some(bot_index) = url.find("/bot") else {
        return url.to_string();
    };
    let token_start = bot_index + 4;
    let Some(relative_end) = url[token_start..].find('/') else {
        return format!("{}<redacted>", &url[..token_start]);
    };
    let token_end = token_start + relative_end;
    format!("{}<redacted>{}", &url[..token_start], &url[token_end..])
}

pub fn redact_webhook_url(url: &str) -> String {
    url.split('?').next().unwrap_or(url).to_string()
}

fn required_string(
    value: &Value,
    key: &str,
    message: &'static str,
) -> Result<String, ExternalConfigError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ExternalConfigError::new(message))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn optional_uuid(value: &Value, key: &str) -> Result<Option<Uuid>, ExternalConfigError> {
    let Some(raw_value) = value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    Uuid::parse_str(raw_value)
        .map(Some)
        .map_err(|_| ExternalConfigError::new(format!("telegram {key} must be a valid uuid")))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::AddressEvent;
    use serde_json::{json, Value};
    use uuid::Uuid;

    use crate::external::{
        build_webhook_request_parts, classify_telegram_response, classify_telegram_verify_response,
        classify_webhook_response, notification_idempotency_key, redact_telegram_url,
        redact_webhook_url, telegram_network_error_message, truncate_provider_response,
        webhook_network_error_message, webhook_signature, TelegramChannelConfig,
        WebhookChannelConfig,
    };

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    fn event() -> AddressEvent {
        AddressEvent {
            id: uuid(11),
            tenant_id: uuid(12),
            chain_id: uuid(13),
            address_id: uuid(14),
            asset_id: uuid(15),
            event_type: "transfer".to_string(),
            direction: "in".to_string(),
            is_transfer: true,
            tx_hash: Some("0xabc".to_string()),
            log_index: Some(0),
            block_number: Some(123),
            block_hash: None,
            confirmations: 12,
            from_address: Some("0xfrom".to_string()),
            to_address: Some("0xto".to_string()),
            amount_raw: Some("1000".to_string()),
            amount_decimal: Some("0.000000000000001".to_string()),
            balance_before_raw: None,
            balance_after_raw: None,
            balance_delta_raw: None,
            metadata: serde_json::json!({}),
            detected_at: Utc.with_ymd_and_hms(2026, 5, 18, 15, 0, 0).unwrap(),
            created_at: Utc.with_ymd_and_hms(2026, 5, 18, 15, 0, 1).unwrap(),
        }
    }

    #[test]
    fn telegram_channel_config_requires_token_reference_and_chat_id() {
        let missing_token = TelegramChannelConfig::parse(&json!({"chat_id": "123"}))
            .expect_err("missing telegram_bot_id and bot_token_env should fail");
        let missing_chat =
            TelegramChannelConfig::parse(&json!({"bot_token_env": "TELEGRAM_BOT_TOKEN"}))
                .expect_err("missing chat_id should fail");

        assert_eq!(
            missing_token.message,
            "telegram telegram_bot_id or bot_token_env is required"
        );
        assert_eq!(missing_chat.message, "telegram chat_id is required");
    }

    #[test]
    fn telegram_channel_config_accepts_bot_id_without_token_env() {
        let bot_id = uuid(42);
        let config = TelegramChannelConfig::parse(&json!({
            "telegram_bot_id": bot_id,
            "chat_id": "123"
        }))
        .expect("valid bot-id telegram config");

        assert_eq!(config.telegram_bot_id, Some(bot_id));
        assert_eq!(config.bot_token_env, None);
        assert_eq!(config.chat_id, "123");
    }

    #[test]
    fn webhook_channel_config_requires_http_url() {
        let missing_url =
            WebhookChannelConfig::parse(&json!({})).expect_err("missing url should fail");
        let invalid_scheme = WebhookChannelConfig::parse(&json!({"url": "ftp://example.com"}))
            .expect_err("non-http url should fail");

        assert_eq!(missing_url.message, "webhook url is required");
        assert_eq!(invalid_scheme.message, "webhook url must use http or https");
    }

    #[test]
    fn webhook_channel_config_defaults_timeout() {
        let config = WebhookChannelConfig::parse(&json!({
            "url": "https://example.com/hook"
        }))
        .expect("valid webhook config");

        assert_eq!(config.timeout_ms, 5000);
        assert_eq!(config.secret_env, None);
    }

    #[test]
    fn webhook_channel_config_rejects_invalid_http_urls() {
        for url in ["https://", "https:///hook", "HTTPS:///hook"] {
            let error = WebhookChannelConfig::parse(&json!({ "url": url }))
                .expect_err("invalid URL should fail");

            assert_eq!(error.message, "webhook url must use http or https");
        }
    }

    #[test]
    fn webhook_channel_config_clamps_timeout() {
        let low = WebhookChannelConfig::parse(&json!({
            "url": "https://example.com/hook",
            "timeout_ms": 999
        }))
        .expect("low timeout config");
        let high = WebhookChannelConfig::parse(&json!({
            "url": "https://example.com/hook",
            "timeout_ms": 30001
        }))
        .expect("high timeout config");
        let explicit = WebhookChannelConfig::parse(&json!({
            "url": "https://example.com/hook",
            "timeout_ms": 7000
        }))
        .expect("explicit timeout config");

        assert_eq!(low.timeout_ms, 1000);
        assert_eq!(high.timeout_ms, 30000);
        assert_eq!(explicit.timeout_ms, 7000);
    }

    #[test]
    fn notification_idempotency_key_is_stable_for_same_rule_channel() {
        let key = notification_idempotency_key(uuid(1), uuid(2), uuid(3), uuid(4));
        let same_key = notification_idempotency_key(uuid(1), uuid(2), uuid(3), uuid(4));

        assert_eq!(key, same_key);
        assert_eq!(
            key,
            "notification:v1:00000000-0000-0000-0000-000000000001:00000000-0000-0000-0000-000000000002:00000000-0000-0000-0000-000000000003:00000000-0000-0000-0000-000000000004"
        );
    }

    #[test]
    fn notification_idempotency_key_changes_for_different_channel() {
        let first = notification_idempotency_key(uuid(1), uuid(2), uuid(3), uuid(4));
        let second = notification_idempotency_key(uuid(1), uuid(2), uuid(3), uuid(5));

        assert_ne!(first, second);
    }

    #[test]
    fn redaction_removes_token_and_webhook_query() {
        assert_eq!(
            redact_telegram_url("https://api.telegram.org/bot123:secret/sendMessage"),
            "https://api.telegram.org/bot<redacted>/sendMessage"
        );
        assert_eq!(
            redact_webhook_url("https://example.com/hook?token=secret"),
            "https://example.com/hook"
        );
    }

    #[test]
    fn webhook_sender_includes_idempotency_headers() {
        let request = build_webhook_request_parts(
            &WebhookChannelConfig {
                url: "https://example.com/hook".to_string(),
                secret_env: None,
                timeout_ms: 5000,
            },
            &event(),
            "notification:key",
            None,
        )
        .expect("build webhook request");

        assert_eq!(request.url, "https://example.com/hook");
        assert_eq!(
            request
                .headers
                .get("X-Coin-Listener-Event-Id")
                .map(String::as_str),
            Some("00000000-0000-0000-0000-00000000000b")
        );
        assert_eq!(
            request
                .headers
                .get("X-Coin-Listener-Idempotency-Key")
                .map(String::as_str),
            Some("notification:key")
        );
        assert!(request
            .body
            .contains("\"idempotency_key\":\"notification:key\""));

        let payload: Value = serde_json::from_str(&request.body).expect("webhook JSON body");
        assert_eq!(payload["idempotency_key"], json!("notification:key"));
        assert_eq!(
            payload["idempotency_key"].as_str(),
            request
                .headers
                .get("X-Coin-Listener-Idempotency-Key")
                .map(String::as_str)
        );
        assert_eq!(
            payload["event_id"],
            json!("00000000-0000-0000-0000-00000000000b")
        );
        assert_eq!(
            payload["tenant_id"],
            json!("00000000-0000-0000-0000-00000000000c")
        );
        assert_eq!(
            payload["chain_id"],
            json!("00000000-0000-0000-0000-00000000000d")
        );
        assert_eq!(
            payload["address_id"],
            json!("00000000-0000-0000-0000-00000000000e")
        );
        assert_eq!(
            payload["asset_id"],
            json!("00000000-0000-0000-0000-00000000000f")
        );
        assert_eq!(payload["event_type"], json!("transfer"));
        assert_eq!(payload["direction"], json!("in"));
        assert_eq!(payload["is_transfer"], json!(true));
        assert_eq!(payload["tx_hash"], json!("0xabc"));
        assert_eq!(payload["block_number"], json!(123));
        assert_eq!(payload["from_address"], json!("0xfrom"));
        assert_eq!(payload["to_address"], json!("0xto"));
        assert_eq!(payload["amount_raw"], json!("1000"));
        assert_eq!(payload["amount_decimal"], json!("0.000000000000001"));
        assert_eq!(payload["detected_at"], json!("2026-05-18T15:00:00+00:00"));
    }

    #[test]
    fn webhook_sender_signs_payload_when_secret_env_is_set() {
        let request = build_webhook_request_parts(
            &WebhookChannelConfig {
                url: "https://example.com/hook".to_string(),
                secret_env: Some("WEBHOOK_SECRET".to_string()),
                timeout_ms: 5000,
            },
            &event(),
            "notification:key",
            Some("secret-value"),
        )
        .expect("build signed webhook request");

        let signature = request
            .headers
            .get("X-Coin-Listener-Signature")
            .expect("signature header");
        assert_eq!(signature.len(), 64);
        assert_eq!(
            signature,
            &webhook_signature("secret-value", request.body.as_bytes())
        );
        assert!(signature
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character)));
    }

    #[test]
    fn webhook_status_classification_distinguishes_retryable_and_permanent_failures() {
        for status_code in [200, 202, 299] {
            assert!(classify_webhook_response(status_code, "accepted").is_sent());
        }
        for status_code in [408, 429, 500, 599] {
            assert!(classify_webhook_response(status_code, "retry").is_transient_failure());
        }
        for status_code in [300, 400, 401, 403, 404, 410, 422] {
            assert!(classify_webhook_response(status_code, "permanent").is_permanent_failure());
        }
    }

    #[test]
    fn webhook_sender_redacts_query_string_from_errors() {
        let message = webhook_network_error_message(
            "https://example.com/hook?token=secret",
            "connection reset for https://example.com/hook?token=secret",
        );

        assert!(message.contains("https://example.com/hook"));
        assert!(!message.contains("token=secret"));
    }

    #[test]
    fn provider_response_truncation_respects_byte_limit() {
        let body = "转".repeat(1000);

        let truncated = truncate_provider_response(&body);

        assert!(truncated.len() <= 2048);
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn telegram_sender_classifies_success_as_sent() {
        let outcome = classify_telegram_response(200, r#"{"ok":true,"result":{"message_id":123}}"#);

        assert!(outcome.is_sent());
        assert_eq!(
            outcome.metadata().provider_message_id.as_deref(),
            Some("123")
        );
        assert_eq!(outcome.metadata().provider_status_code, Some(200));
    }

    #[test]
    fn telegram_verify_success_uses_get_me_ok_response() {
        let sender = crate::external::ExternalNotificationSender::with_telegram_api_base_url(
            reqwest::Client::new(),
            "https://telegram.example.test".to_string(),
        );
        let outcome = classify_telegram_verify_response(
            200,
            r#"{
                "ok" : true,
                "result" : { "id" : 123, "is_bot" : true }
            }"#,
        );

        assert_eq!(
            sender.telegram_bot_method_url("123:secret", "getMe"),
            "https://telegram.example.test/bot123:secret/getMe"
        );
        assert!(outcome.is_sent());
        assert_eq!(outcome.metadata().provider_message_id, None);
        assert_eq!(outcome.metadata().provider_status_code, Some(200));
    }

    #[test]
    fn telegram_sender_classifies_rate_limit_as_retryable() {
        let outcome =
            classify_telegram_response(429, r#"{"ok":false,"description":"Too Many Requests"}"#);

        assert!(outcome.is_transient_failure());
        assert!(outcome
            .metadata()
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("retryable status 429"));
    }

    #[test]
    fn telegram_sender_classifies_auth_error_as_permanent() {
        let outcome =
            classify_telegram_response(401, r#"{"ok":false,"description":"Unauthorized"}"#);

        assert!(outcome.is_permanent_failure());
        assert!(outcome
            .metadata()
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("permanent status 401"));
    }

    #[test]
    fn telegram_sender_retries_unparsable_success_body() {
        let outcome = classify_telegram_response(200, "not-json");

        assert!(outcome.is_transient_failure());
        assert!(outcome
            .metadata()
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("unparsable success body"));
    }

    #[test]
    fn telegram_sender_retries_unsuccessful_success_body() {
        let outcome = classify_telegram_response(
            200,
            r#"{"ok":false,"description":"Bad Request: chat not found"}"#,
        );

        assert!(outcome.is_transient_failure());
        assert!(outcome
            .metadata()
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("unsuccessful success body"));
    }

    #[test]
    fn telegram_request_url_redacts_token_from_error_message() {
        let error = telegram_network_error_message(
            "https://api.telegram.org/bot123:secret/sendMessage",
            "timeout",
        );

        assert!(error.contains("bot<redacted>"));
        assert!(!error.contains("123:secret"));
    }
}
