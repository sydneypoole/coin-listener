use serde_json::Value;
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
    pub bot_token_env: String,
    pub chat_id: String,
}

impl TelegramChannelConfig {
    pub fn parse(value: &Value) -> Result<Self, ExternalConfigError> {
        let bot_token_env =
            required_string(value, "bot_token_env", "telegram bot_token_env is required")?;
        let chat_id = required_string(value, "chat_id", "telegram chat_id is required")?;
        Ok(Self {
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

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use crate::external::{
        notification_idempotency_key, redact_telegram_url, redact_webhook_url,
        TelegramChannelConfig, WebhookChannelConfig,
    };

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    #[test]
    fn telegram_channel_config_requires_token_env_and_chat_id() {
        let missing_token = TelegramChannelConfig::parse(&json!({"chat_id": "123"}))
            .expect_err("missing bot_token_env should fail");
        let missing_chat =
            TelegramChannelConfig::parse(&json!({"bot_token_env": "TELEGRAM_BOT_TOKEN"}))
                .expect_err("missing chat_id should fail");

        assert_eq!(missing_token.message, "telegram bot_token_env is required");
        assert_eq!(missing_chat.message, "telegram chat_id is required");
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
}
