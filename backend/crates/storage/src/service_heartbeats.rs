use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use chrono::{DateTime, Duration, Utc};
use coin_listener_core::{
    models::{ServiceHealthStatus, ServiceHeartbeatStatusItem},
    AppError, AppResult,
};
use serde_json::json;
use sqlx::{FromRow, PgPool};
use tokio::time::{self, Duration as TokioDuration};
use tracing::{error, info};
use uuid::Uuid;

pub const SERVICE_HEARTBEAT_STALE_SECONDS: i64 = 90;
pub const SERVICE_HEARTBEAT_INTERVAL_SECONDS: i64 = 30;

pub const UPSERT_SERVICE_HEARTBEAT_QUERY: &str = r#"
INSERT INTO service_heartbeats (
    service_name, instance_id, status, started_at, last_seen_at, metadata
)
VALUES ($1, $2, 'online', $3, $4, $5)
ON CONFLICT (service_name, instance_id) DO UPDATE
SET status = 'online',
    last_seen_at = EXCLUDED.last_seen_at,
    metadata = EXCLUDED.metadata,
    updated_at = NOW()
RETURNING service_name, instance_id, status, started_at, last_seen_at, metadata,
          created_at, updated_at
"#;

pub const LIST_SERVICE_HEARTBEATS_QUERY: &str = r#"
SELECT service_name, instance_id, status, started_at, last_seen_at, metadata,
       created_at, updated_at
FROM service_heartbeats
ORDER BY service_name ASC, last_seen_at DESC
"#;

#[derive(Debug, Clone, FromRow)]
pub struct ServiceHeartbeatRow {
    pub service_name: String,
    pub instance_id: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn validate_service_heartbeat(service_name: &str, instance_id: &str) -> AppResult<()> {
    if !matches!(
        service_name,
        "api-server" | "scheduler" | "worker" | "notifier"
    ) {
        return Err(AppError::Validation("unknown service_name".to_string()));
    }
    if instance_id.trim().is_empty() {
        return Err(AppError::Validation("instance_id is required".to_string()));
    }
    Ok(())
}

pub fn service_heartbeat_is_stale(last_seen_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    last_seen_at < now - Duration::seconds(SERVICE_HEARTBEAT_STALE_SECONDS)
}

pub fn heartbeat_metadata() -> serde_json::Value {
    json!({
        "pid": std::process::id(),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

pub fn service_heartbeat_instance_id() -> String {
    Uuid::new_v4().to_string()
}

pub async fn upsert_service_heartbeat(
    pool: &PgPool,
    service_name: &str,
    instance_id: &str,
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
    metadata: serde_json::Value,
) -> AppResult<ServiceHeartbeatRow> {
    validate_service_heartbeat(service_name, instance_id)?;

    sqlx::query_as::<_, ServiceHeartbeatRow>(UPSERT_SERVICE_HEARTBEAT_QUERY)
        .bind(service_name)
        .bind(instance_id)
        .bind(started_at)
        .bind(now)
        .bind(metadata)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn list_service_heartbeats(pool: &PgPool) -> AppResult<Vec<ServiceHeartbeatRow>> {
    sqlx::query_as::<_, ServiceHeartbeatRow>(LIST_SERVICE_HEARTBEATS_QUERY)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn write_service_heartbeat_tick(
    pool: &PgPool,
    service_name: &'static str,
    instance_id: &str,
    started_at: DateTime<Utc>,
) -> AppResult<ServiceHeartbeatRow> {
    upsert_service_heartbeat(
        pool,
        service_name,
        instance_id,
        started_at,
        Utc::now(),
        heartbeat_metadata(),
    )
    .await
}

pub async fn run_service_heartbeat(
    pool: PgPool,
    service_name: &'static str,
    instance_id: String,
    started_at: DateTime<Utc>,
    shutdown: Arc<AtomicBool>,
) {
    loop {
        match write_service_heartbeat_tick(&pool, service_name, &instance_id, started_at).await {
            Ok(_) => {
                info!(service = service_name, instance_id = %instance_id, "service heartbeat written")
            }
            Err(error) => {
                error!(service = service_name, instance_id = %instance_id, error = %error, "service heartbeat write failed")
            }
        }

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        time::sleep(TokioDuration::from_secs(
            SERVICE_HEARTBEAT_INTERVAL_SECONDS as u64,
        ))
        .await;
    }
}

pub async fn system_service_health(
    pool: &PgPool,
    now: DateTime<Utc>,
) -> AppResult<ServiceHealthStatus> {
    let rows = list_service_heartbeats(pool).await?;
    let mut online = 0;
    let mut stale = 0;
    let items = rows
        .into_iter()
        .map(|row| {
            let is_stale = service_heartbeat_is_stale(row.last_seen_at, now);
            if is_stale {
                stale += 1;
            } else {
                online += 1;
            }
            ServiceHeartbeatStatusItem {
                service_name: row.service_name,
                instance_id: row.instance_id,
                status: row.status,
                started_at: row.started_at,
                last_seen_at: row.last_seen_at,
                stale_after_seconds: SERVICE_HEARTBEAT_STALE_SECONDS,
                is_stale,
                metadata: row.metadata,
            }
        })
        .collect();

    Ok(ServiceHealthStatus {
        online,
        stale,
        items,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use coin_listener_core::AppError;

    use crate::service_heartbeats::{
        heartbeat_metadata, service_heartbeat_instance_id, service_heartbeat_is_stale,
        validate_service_heartbeat, SERVICE_HEARTBEAT_INTERVAL_SECONDS,
        SERVICE_HEARTBEAT_STALE_SECONDS, UPSERT_SERVICE_HEARTBEAT_QUERY,
    };

    #[test]
    fn service_heartbeat_instance_id_is_uuid_shaped() {
        let instance_id = service_heartbeat_instance_id();

        uuid::Uuid::parse_str(&instance_id).expect("uuid instance id");
    }

    #[test]
    fn heartbeat_interval_is_shorter_than_stale_threshold() {
        assert_eq!(SERVICE_HEARTBEAT_INTERVAL_SECONDS, 30);
        assert!(SERVICE_HEARTBEAT_INTERVAL_SECONDS < SERVICE_HEARTBEAT_STALE_SECONDS);
    }

    #[test]
    fn heartbeat_validation_accepts_known_services() {
        for service in ["api-server", "scheduler", "worker", "notifier"] {
            validate_service_heartbeat(service, "instance-1").expect(service);
        }
    }

    #[test]
    fn heartbeat_validation_rejects_unknown_service() {
        let result = validate_service_heartbeat("unknown", "instance-1");

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "unknown service_name"
        ));
    }

    #[test]
    fn heartbeat_validation_rejects_empty_instance_id() {
        let result = validate_service_heartbeat("worker", "   ");

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message == "instance_id is required"
        ));
    }

    #[test]
    fn heartbeat_stale_classification_uses_ninety_second_threshold() {
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 2, 0).unwrap();
        let fresh = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 31).unwrap();
        let stale = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 29).unwrap();

        assert_eq!(SERVICE_HEARTBEAT_STALE_SECONDS, 90);
        assert!(!service_heartbeat_is_stale(fresh, now));
        assert!(service_heartbeat_is_stale(stale, now));
    }

    #[test]
    fn heartbeat_metadata_contains_only_safe_runtime_fields() {
        let metadata = heartbeat_metadata();

        assert!(metadata.get("pid").is_some());
        assert_eq!(
            metadata.get("version").and_then(|value| value.as_str()),
            Some("0.1.0")
        );
        assert!(metadata.get("database_url").is_none());
        assert!(metadata.get("token").is_none());
    }

    #[test]
    fn upsert_heartbeat_query_preserves_started_at_on_conflict() {
        assert!(UPSERT_SERVICE_HEARTBEAT_QUERY.contains("ON CONFLICT (service_name, instance_id)"));
        assert!(UPSERT_SERVICE_HEARTBEAT_QUERY.contains("last_seen_at = EXCLUDED.last_seen_at"));
        assert!(!UPSERT_SERVICE_HEARTBEAT_QUERY.contains("started_at = EXCLUDED.started_at"));
    }

    #[test]
    fn service_heartbeat_migration_defines_primary_key_and_indexes() {
        let migration = include_str!("../migrations/0010_service_heartbeats.sql");

        assert!(migration.contains("CREATE TABLE IF NOT EXISTS service_heartbeats"));
        assert!(migration.contains("PRIMARY KEY (service_name, instance_id)"));
        assert!(migration.contains("idx_service_heartbeats_last_seen"));
        assert!(migration.contains("idx_service_heartbeats_service"));
    }
}
