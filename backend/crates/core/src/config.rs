use figment::Figment;
use serde::{de::DeserializeOwned, Deserialize};
use std::{env, str::FromStr};

use crate::{AppError, AppResult};

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub postgres: PostgresConfig,
    pub redis: RedisConfig,
    pub scan: ScanConfig,
    pub notify: NotifyConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    pub token_secret: String,
    pub token_ttl_seconds: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub enable_dev_routes: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostgresConfig {
    pub database_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub redis_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanConfig {
    pub scheduler_tick_seconds: u64,
    pub scheduler_batch_size: i64,
    pub queue_key: String,
    pub lock_ttl_seconds: u64,
    pub job_max_attempts: i32,
    pub job_idle_sleep_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotifyConfig {
    pub queue_key: String,
    pub outbox_batch_size: i64,
    pub outbox_max_attempts: i32,
    pub outbox_stale_lock_seconds: i64,
    pub outbox_idle_sleep_ms: u64,
    pub telegram_webhook_secret: Option<String>,
}

impl AppConfig {
    pub fn from_env() -> AppResult<Self> {
        Figment::new()
            .merge(("server.host", env_string("API_SERVER_HOST", "0.0.0.0")))
            .merge(("server.port", env_parse("API_SERVER_PORT", 8080)?))
            .merge((
                "server.enable_dev_routes",
                env_parse("ENABLE_DEV_ROUTES", false)?,
            ))
            .merge((
                "postgres.database_url",
                env_string(
                    "DATABASE_URL",
                    "postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener",
                ),
            ))
            .merge((
                "redis.redis_url",
                env_string("REDIS_URL", "redis://localhost:6379"),
            ))
            .merge((
                "scan.scheduler_tick_seconds",
                env_parse("SCHEDULER_TICK_SECONDS", 30_u64)?,
            ))
            .merge((
                "scan.scheduler_batch_size",
                env_parse("SCHEDULER_BATCH_SIZE", 100_i64)?,
            ))
            .merge((
                "scan.queue_key",
                env_string("SCAN_QUEUE_KEY", "scan:address:queue"),
            ))
            .merge((
                "scan.lock_ttl_seconds",
                env_parse("SCAN_LOCK_TTL_SECONDS", 120_usize)?,
            ))
            .merge((
                "scan.job_max_attempts",
                env_parse("SCAN_JOB_MAX_ATTEMPTS", 10_i32)?,
            ))
            .merge((
                "scan.job_idle_sleep_ms",
                env_parse("SCAN_JOB_IDLE_SLEEP_MS", 500_u64)?,
            ))
            .merge((
                "notify.queue_key",
                env_string("NOTIFY_QUEUE_KEY", "notify:event:queue"),
            ))
            .merge((
                "notify.outbox_batch_size",
                env_parse("NOTIFICATION_OUTBOX_BATCH_SIZE", 50_i64)?,
            ))
            .merge((
                "notify.outbox_max_attempts",
                env_parse("NOTIFICATION_OUTBOX_MAX_ATTEMPTS", 10_i32)?,
            ))
            .merge((
                "notify.outbox_stale_lock_seconds",
                env_parse("NOTIFICATION_OUTBOX_STALE_LOCK_SECONDS", 300_i64)?,
            ))
            .merge((
                "notify.outbox_idle_sleep_ms",
                env_parse("NOTIFICATION_OUTBOX_IDLE_SLEEP_MS", 500_u64)?,
            ))
            .merge((
                "notify.telegram_webhook_secret",
                env_optional_string("TELEGRAM_WEBHOOK_SECRET"),
            ))
            .merge(("auth.token_secret", env_string("AUTH_TOKEN_SECRET", "")))
            .merge((
                "auth.token_ttl_seconds",
                env_parse("AUTH_TOKEN_TTL_SECONDS", 43_200_i64)?,
            ))
            .extract()
            .map_err(|error| AppError::Config(error.to_string()))
            .and_then(validate_config)
    }

    pub fn server_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}

fn validate_config(config: AppConfig) -> AppResult<AppConfig> {
    if config.scan.job_max_attempts <= 0 {
        return Err(AppError::Config(
            "SCAN_JOB_MAX_ATTEMPTS must be positive".to_string(),
        ));
    }
    if config.scan.lock_ttl_seconds == 0 {
        return Err(AppError::Config(
            "SCAN_LOCK_TTL_SECONDS must be positive".to_string(),
        ));
    }
    if i64::try_from(config.scan.lock_ttl_seconds).is_err() {
        return Err(AppError::Config(
            "SCAN_LOCK_TTL_SECONDS must fit in i64".to_string(),
        ));
    }
    Ok(config)
}

fn env_string(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn env_optional_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_parse<T>(name: &str, default: T) -> AppResult<T>
where
    T: Copy + DeserializeOwned + FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value
            .parse::<T>()
            .map_err(|error| AppError::Config(format!("invalid {name}: {error}"))),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{AppConfig, AuthConfig, NotifyConfig};

    #[test]
    fn auth_config_carries_token_runtime_settings() {
        let config = AuthConfig {
            token_secret: "test-secret-with-enough-entropy".to_string(),
            token_ttl_seconds: 43_200,
        };

        assert_eq!(config.token_secret, "test-secret-with-enough-entropy");
        assert_eq!(config.token_ttl_seconds, 43_200);
    }

    #[test]
    fn notify_config_carries_outbox_runtime_settings() {
        let config = NotifyConfig {
            queue_key: "notify:event:queue".to_string(),
            outbox_batch_size: 50,
            outbox_max_attempts: 10,
            outbox_stale_lock_seconds: 300,
            outbox_idle_sleep_ms: 500,
            telegram_webhook_secret: None,
        };

        assert_eq!(config.queue_key, "notify:event:queue");
        assert_eq!(config.outbox_batch_size, 50);
        assert_eq!(config.outbox_max_attempts, 10);
        assert_eq!(config.outbox_stale_lock_seconds, 300);
        assert_eq!(config.outbox_idle_sleep_ms, 500);
        assert_eq!(config.telegram_webhook_secret, None);
    }

    #[test]
    fn app_config_loads_telegram_webhook_secret_with_blank_as_none() {
        let previous = std::env::var_os("TELEGRAM_WEBHOOK_SECRET");

        std::env::set_var("TELEGRAM_WEBHOOK_SECRET", "webhook-secret");
        let configured = AppConfig::from_env().expect("config loads with telegram webhook secret");
        assert_eq!(
            configured.notify.telegram_webhook_secret.as_deref(),
            Some("webhook-secret")
        );

        std::env::set_var("TELEGRAM_WEBHOOK_SECRET", "  ");
        let blank = AppConfig::from_env().expect("config loads with blank telegram webhook secret");
        assert_eq!(blank.notify.telegram_webhook_secret, None);

        match previous {
            Some(value) => std::env::set_var("TELEGRAM_WEBHOOK_SECRET", value),
            None => std::env::remove_var("TELEGRAM_WEBHOOK_SECRET"),
        }
    }

    fn test_app_config() -> AppConfig {
        AppConfig {
            server: super::ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
                enable_dev_routes: false,
            },
            postgres: super::PostgresConfig {
                database_url: "postgres://localhost/test".to_string(),
            },
            redis: super::RedisConfig {
                redis_url: "redis://localhost:6379".to_string(),
            },
            scan: super::ScanConfig {
                scheduler_tick_seconds: 30,
                scheduler_batch_size: 100,
                queue_key: "scan:address:queue".to_string(),
                lock_ttl_seconds: 120,
                job_max_attempts: 10,
                job_idle_sleep_ms: 500,
            },
            notify: NotifyConfig {
                queue_key: "notify:event:queue".to_string(),
                outbox_batch_size: 50,
                outbox_max_attempts: 10,
                outbox_stale_lock_seconds: 300,
                outbox_idle_sleep_ms: 500,
                telegram_webhook_secret: None,
            },
            auth: AuthConfig {
                token_secret: "test-secret-with-enough-entropy".to_string(),
                token_ttl_seconds: 43_200,
            },
        }
    }

    #[test]
    fn scan_job_max_attempts_must_be_positive() {
        let mut config = test_app_config();
        config.scan.job_max_attempts = 0;

        let error = super::validate_config(config).expect_err("zero max attempts is invalid");
        assert!(error
            .to_string()
            .contains("SCAN_JOB_MAX_ATTEMPTS must be positive"));
    }

    #[test]
    fn scan_lock_ttl_seconds_must_be_positive() {
        let mut config = test_app_config();
        config.scan.lock_ttl_seconds = 0;

        let error = super::validate_config(config).expect_err("zero scan lock TTL is invalid");
        assert!(error
            .to_string()
            .contains("SCAN_LOCK_TTL_SECONDS must be positive"));
    }

    #[test]
    fn scan_lock_ttl_seconds_must_fit_sql_interval_binding() {
        let mut config = test_app_config();
        config.scan.lock_ttl_seconds = (i64::MAX as u64) + 1;

        let error = super::validate_config(config).expect_err("oversized scan lock TTL is invalid");
        assert!(error
            .to_string()
            .contains("SCAN_LOCK_TTL_SECONDS must fit in i64"));
    }

    #[test]
    fn app_config_parses_numeric_environment_values() {
        std::env::set_var("AUTH_TOKEN_TTL_SECONDS", "43200");
        std::env::set_var("API_SERVER_PORT", "8080");
        std::env::set_var("ENABLE_DEV_ROUTES", "false");
        std::env::set_var("SCHEDULER_TICK_SECONDS", "30");
        std::env::set_var("SCHEDULER_BATCH_SIZE", "100");
        std::env::set_var("SCAN_LOCK_TTL_SECONDS", "120");
        std::env::set_var("SCAN_JOB_MAX_ATTEMPTS", "10");
        std::env::set_var("SCAN_JOB_IDLE_SLEEP_MS", "500");
        std::env::set_var("NOTIFICATION_OUTBOX_BATCH_SIZE", "50");
        std::env::set_var("NOTIFICATION_OUTBOX_MAX_ATTEMPTS", "10");
        std::env::set_var("NOTIFICATION_OUTBOX_STALE_LOCK_SECONDS", "300");
        std::env::set_var("NOTIFICATION_OUTBOX_IDLE_SLEEP_MS", "500");

        let config = AppConfig::from_env().expect("numeric env values parse");

        assert_eq!(config.auth.token_ttl_seconds, 43_200);
        assert_eq!(config.server.port, 8080);
        assert!(!config.server.enable_dev_routes);
        assert_eq!(config.scan.scheduler_tick_seconds, 30);
        assert_eq!(config.scan.scheduler_batch_size, 100);
        assert_eq!(config.scan.lock_ttl_seconds, 120);
        assert_eq!(config.scan.job_max_attempts, 10);
        assert_eq!(config.scan.job_idle_sleep_ms, 500);
        assert_eq!(config.notify.outbox_batch_size, 50);
        assert_eq!(config.notify.outbox_max_attempts, 10);
        assert_eq!(config.notify.outbox_stale_lock_seconds, 300);
        assert_eq!(config.notify.outbox_idle_sleep_ms, 500);
    }
}
