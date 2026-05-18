use figment::Figment;
use serde::Deserialize;
use std::env;

use crate::{AppError, AppResult};

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub postgres: PostgresConfig,
    pub redis: RedisConfig,
    pub scan: ScanConfig,
    pub notify: NotifyConfig,
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
    pub lock_ttl_seconds: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotifyConfig {
    pub queue_key: String,
    pub outbox_batch_size: i64,
    pub outbox_max_attempts: i32,
    pub outbox_stale_lock_seconds: i64,
    pub outbox_idle_sleep_ms: u64,
}

impl AppConfig {
    pub fn from_env() -> AppResult<Self> {
        Figment::new()
            .merge(("server.host", env::var("API_SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string())))
            .merge(("server.port", env::var("API_SERVER_PORT").unwrap_or_else(|_| "8080".to_string())))
            .merge(("server.enable_dev_routes", env::var("ENABLE_DEV_ROUTES").unwrap_or_else(|_| "false".to_string())))
            .merge(("postgres.database_url", env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener".to_string()
            })))
            .merge(("redis.redis_url", env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string())))
            .merge((
                "scan.scheduler_tick_seconds",
                env::var("SCHEDULER_TICK_SECONDS").unwrap_or_else(|_| "30".to_string()),
            ))
            .merge((
                "scan.scheduler_batch_size",
                env::var("SCHEDULER_BATCH_SIZE").unwrap_or_else(|_| "100".to_string()),
            ))
            .merge((
                "scan.queue_key",
                env::var("SCAN_QUEUE_KEY").unwrap_or_else(|_| "scan:address:queue".to_string()),
            ))
            .merge((
                "scan.lock_ttl_seconds",
                env::var("SCAN_LOCK_TTL_SECONDS").unwrap_or_else(|_| "120".to_string()),
            ))
            .merge((
                "notify.queue_key",
                env::var("NOTIFY_QUEUE_KEY").unwrap_or_else(|_| "notify:event:queue".to_string()),
            ))
            .merge((
                "notify.outbox_batch_size",
                env::var("NOTIFICATION_OUTBOX_BATCH_SIZE").unwrap_or_else(|_| "50".to_string()),
            ))
            .merge((
                "notify.outbox_max_attempts",
                env::var("NOTIFICATION_OUTBOX_MAX_ATTEMPTS").unwrap_or_else(|_| "10".to_string()),
            ))
            .merge((
                "notify.outbox_stale_lock_seconds",
                env::var("NOTIFICATION_OUTBOX_STALE_LOCK_SECONDS")
                    .unwrap_or_else(|_| "300".to_string()),
            ))
            .merge((
                "notify.outbox_idle_sleep_ms",
                env::var("NOTIFICATION_OUTBOX_IDLE_SLEEP_MS").unwrap_or_else(|_| "500".to_string()),
            ))
            .extract()
            .map_err(|error| AppError::Config(error.to_string()))
    }

    pub fn server_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}

#[cfg(test)]
mod tests {
    use crate::config::NotifyConfig;

    #[test]
    fn notify_config_carries_outbox_runtime_settings() {
        let config = NotifyConfig {
            queue_key: "notify:event:queue".to_string(),
            outbox_batch_size: 50,
            outbox_max_attempts: 10,
            outbox_stale_lock_seconds: 300,
            outbox_idle_sleep_ms: 500,
        };

        assert_eq!(config.queue_key, "notify:event:queue");
        assert_eq!(config.outbox_batch_size, 50);
        assert_eq!(config.outbox_max_attempts, 10);
        assert_eq!(config.outbox_stale_lock_seconds, 300);
        assert_eq!(config.outbox_idle_sleep_ms, 500);
    }
}
