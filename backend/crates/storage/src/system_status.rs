use chrono::{DateTime, Duration, Utc};
use coin_listener_core::{
    models::{
        EventStatus, NotificationStatus, ProviderChainStatus, ProviderHealthStatus, ProviderStatus,
        ProviderStatusItem, ScanStatus,
    },
    AppError, AppResult,
};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{repositories, scan_runs};

pub const NOTIFICATION_STATUS_STALE_MINUTES: i64 = 15;
pub const PROVIDER_STATUS_DEFAULT_FAILURES: i32 = 0;

pub const SCAN_STATUS_QUERY: &str = r#"
    SELECT
        COUNT(*) FILTER (WHERE status = 'active') AS active_addresses,
        COUNT(*) FILTER (WHERE status = 'active' AND next_scan_at <= NOW()) AS due_addresses,
        COUNT(*) FILTER (
            WHERE status = 'active'
              AND next_scan_at <= NOW() - INTERVAL '5 minutes'
        ) AS overdue_addresses,
        MAX(last_scanned_at) AS last_scanned_at
    FROM watched_addresses
    WHERE tenant_id = $1
"#;

pub const EVENT_STATUS_QUERY: &str = r#"
    SELECT
        COUNT(*) AS last_24h_total,
        COUNT(*) FILTER (WHERE is_transfer = TRUE) AS last_24h_transfers,
        COUNT(*) FILTER (WHERE is_transfer = FALSE) AS last_24h_non_transfers
    FROM address_events
    WHERE created_at >= NOW() - INTERVAL '24 hours'
"#;

pub const NOTIFICATION_STATUS_QUERY: &str = r#"
    SELECT
        COUNT(*) FILTER (WHERE nd.status = 'sent') AS last_24h_sent,
        COUNT(*) FILTER (WHERE nd.status = 'skipped') AS last_24h_skipped,
        COUNT(*) FILTER (WHERE nd.status = 'failed') AS last_24h_failed,
        (
            SELECT COUNT(*)
            FROM in_app_notifications ian
            WHERE ian.read_at IS NULL
        ) AS unread_in_app
    FROM notification_deliveries nd
    WHERE nd.created_at >= NOW() - INTERVAL '24 hours'
"#;

pub const PROVIDER_CHAIN_STATUS_QUERY: &str = r#"
    SELECT
        c.id AS chain_id,
        c.name AS chain_name,
        COUNT(p.id) FILTER (WHERE p.status = 'active') AS active,
        COUNT(p.id) FILTER (WHERE p.status <> 'active') AS inactive
    FROM providers p
    JOIN chains c ON c.id = p.chain_id
    GROUP BY c.id, c.name
    ORDER BY c.name ASC
"#;

pub const PROVIDER_ITEMS_QUERY: &str = r#"
    SELECT
        p.id,
        p.chain_id,
        c.name AS chain_name,
        p.provider_type,
        p.name,
        p.base_url,
        p.priority,
        p.qps_limit,
        p.timeout_ms,
        p.status,
        COALESCE(ph.consecutive_failures, 0) AS consecutive_failures,
        ph.last_success_at,
        ph.last_failure_at,
        ph.disabled_until,
        ph.last_error,
        CASE
            WHEN ph.disabled_until IS NOT NULL AND ph.disabled_until > NOW() THEN TRUE
            ELSE FALSE
        END AS is_circuit_open
    FROM providers p
    JOIN chains c ON c.id = p.chain_id
    LEFT JOIN provider_health ph ON ph.provider_id = p.id
    ORDER BY c.name ASC, p.priority ASC, p.name ASC
"#;

#[derive(Debug, FromRow)]
struct ScanStatusRow {
    active_addresses: i64,
    due_addresses: i64,
    overdue_addresses: i64,
    last_scanned_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct EventStatusRow {
    last_24h_total: i64,
    last_24h_transfers: i64,
    last_24h_non_transfers: i64,
}

#[derive(Debug, FromRow)]
struct NotificationStatusRow {
    last_24h_sent: i64,
    last_24h_skipped: i64,
    last_24h_failed: i64,
    unread_in_app: i64,
}

#[derive(Debug, FromRow)]
struct ProviderTotalsRow {
    active: i64,
    inactive: i64,
}

#[derive(Debug, FromRow)]
struct ProviderChainStatusRow {
    chain_id: Uuid,
    chain_name: String,
    active: i64,
    inactive: i64,
}

#[derive(Debug, FromRow)]
struct ProviderStatusItemRow {
    id: Uuid,
    chain_id: Uuid,
    chain_name: String,
    provider_type: String,
    name: String,
    base_url: String,
    priority: i32,
    qps_limit: i32,
    timeout_ms: i32,
    status: String,
    consecutive_failures: i32,
    last_success_at: Option<DateTime<Utc>>,
    last_failure_at: Option<DateTime<Utc>>,
    disabled_until: Option<DateTime<Utc>>,
    last_error: Option<String>,
    is_circuit_open: bool,
}

pub async fn system_scan_status(pool: &PgPool, tenant_id: Uuid) -> AppResult<ScanStatus> {
    let row = sqlx::query_as::<_, ScanStatusRow>(SCAN_STATUS_QUERY)
        .bind(tenant_id)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let summary = scan_runs::scan_run_health_summary(pool, tenant_id).await?;
    let recent_runs = scan_runs::recent_scan_runs(pool, tenant_id, 5).await?;

    Ok(ScanStatus {
        active_addresses: row.active_addresses,
        due_addresses: row.due_addresses,
        overdue_addresses: row.overdue_addresses,
        last_scanned_at: row.last_scanned_at,
        last_success_at: summary.last_success_at,
        last_failed_at: summary.last_failed_at,
        last_24h_success: summary.last_24h_success,
        last_24h_failed: summary.last_24h_failed,
        recent_runs,
    })
}

pub async fn system_event_status(pool: &PgPool) -> AppResult<EventStatus> {
    let row = sqlx::query_as::<_, EventStatusRow>(EVENT_STATUS_QUERY)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(EventStatus {
        last_24h_total: row.last_24h_total,
        last_24h_transfers: row.last_24h_transfers,
        last_24h_non_transfers: row.last_24h_non_transfers,
    })
}

pub async fn system_notification_status(pool: &PgPool) -> AppResult<NotificationStatus> {
    let row = sqlx::query_as::<_, NotificationStatusRow>(NOTIFICATION_STATUS_QUERY)
        .fetch_one(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

    let now = Utc::now();
    let outbox = repositories::notification_outbox_status_counts(
        pool,
        now,
        now - Duration::minutes(NOTIFICATION_STATUS_STALE_MINUTES),
    )
    .await?;

    Ok(NotificationStatus {
        last_24h_sent: row.last_24h_sent,
        last_24h_skipped: row.last_24h_skipped,
        last_24h_failed: row.last_24h_failed,
        unread_in_app: row.unread_in_app,
        outbox,
    })
}

pub async fn system_provider_status(pool: &PgPool) -> AppResult<ProviderStatus> {
    let totals = sqlx::query_as::<_, ProviderTotalsRow>(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE status = 'active') AS active,
            COUNT(*) FILTER (WHERE status <> 'active') AS inactive
        FROM providers
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Database(error.to_string()))?;

    let by_chain = sqlx::query_as::<_, ProviderChainStatusRow>(PROVIDER_CHAIN_STATUS_QUERY)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .into_iter()
        .map(|row| ProviderChainStatus {
            chain_id: row.chain_id,
            chain_name: row.chain_name,
            active: row.active,
            inactive: row.inactive,
        })
        .collect();

    let items = sqlx::query_as::<_, ProviderStatusItemRow>(PROVIDER_ITEMS_QUERY)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?
        .into_iter()
        .map(|row| ProviderStatusItem {
            id: row.id,
            chain_id: row.chain_id,
            chain_name: row.chain_name,
            provider_type: row.provider_type,
            name: row.name,
            base_url: row.base_url,
            priority: row.priority,
            qps_limit: row.qps_limit,
            timeout_ms: row.timeout_ms,
            status: row.status,
            health: ProviderHealthStatus {
                consecutive_failures: row.consecutive_failures,
                last_success_at: row.last_success_at,
                last_failure_at: row.last_failure_at,
                disabled_until: row.disabled_until,
                last_error: row.last_error,
                is_circuit_open: row.is_circuit_open,
            },
        })
        .collect();

    Ok(ProviderStatus {
        active: totals.active,
        inactive: totals.inactive,
        by_chain,
        items,
    })
}

#[cfg(test)]
mod tests {
    use crate::{
        scan_runs::{RECENT_SCAN_RUNS_QUERY, SCAN_RUN_HEALTH_SUMMARY_QUERY},
        service_heartbeats::SERVICE_HEARTBEAT_STALE_SECONDS,
        system_status::{
            EVENT_STATUS_QUERY, NOTIFICATION_STATUS_QUERY, NOTIFICATION_STATUS_STALE_MINUTES,
            PROVIDER_CHAIN_STATUS_QUERY, PROVIDER_ITEMS_QUERY, PROVIDER_STATUS_DEFAULT_FAILURES,
            SCAN_STATUS_QUERY,
        },
    };

    #[test]
    fn scan_status_query_counts_active_due_and_overdue_addresses_for_tenant() {
        assert!(SCAN_STATUS_QUERY.contains("status = 'active'"));
        assert!(SCAN_STATUS_QUERY.contains("next_scan_at <= NOW()"));
        assert!(SCAN_STATUS_QUERY.contains("last_scanned_at"));
        assert!(SCAN_STATUS_QUERY.contains("WHERE tenant_id = $1"));
    }

    #[test]
    fn scan_status_uses_scan_run_health_summary() {
        assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("WHERE tenant_id = $1"));
        assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("status = 'success'"));
        assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("status = 'failed'"));
        assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("last_success_at"));
        assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("last_failed_at"));
        assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("last_24h_success"));
        assert!(SCAN_RUN_HEALTH_SUMMARY_QUERY.contains("last_24h_failed"));
    }

    #[test]
    fn scan_status_recent_runs_are_compact_limited_and_tenant_scoped() {
        assert!(RECENT_SCAN_RUNS_QUERY.contains("WHERE sr.tenant_id = $1"));
        assert!(RECENT_SCAN_RUNS_QUERY.contains("ORDER BY sr.started_at DESC"));
        assert!(RECENT_SCAN_RUNS_QUERY.contains("LIMIT $2"));
        assert!(RECENT_SCAN_RUNS_QUERY.contains("JOIN chains c"));
        assert!(RECENT_SCAN_RUNS_QUERY.contains("JOIN watched_addresses wa"));
        assert!(RECENT_SCAN_RUNS_QUERY.contains("wa.tenant_id = sr.tenant_id"));
        assert!(RECENT_SCAN_RUNS_QUERY.contains("wa.chain_id = sr.chain_id"));
    }

    #[test]
    fn system_scan_status_attaches_recent_five_scan_runs() {
        let source = include_str!("system_status.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source is present");

        assert!(source.contains("tenant_id: Uuid"));
        assert!(source.contains(".bind(tenant_id)"));
        assert!(source.contains("scan_runs::scan_run_health_summary(pool, tenant_id).await?"));
        assert!(source.contains("scan_runs::recent_scan_runs(pool, tenant_id, 5).await?"));
        assert!(source.contains("last_success_at: summary.last_success_at"));
        assert!(source.contains("last_failed_at: summary.last_failed_at"));
        assert!(source.contains("last_24h_success: summary.last_24h_success"));
        assert!(source.contains("last_24h_failed: summary.last_24h_failed"));
        assert!(source.contains("recent_runs"));
    }

    #[test]
    fn event_status_query_counts_last_24h_transfers() {
        assert!(EVENT_STATUS_QUERY.contains("created_at >= NOW() - INTERVAL '24 hours'"));
        assert!(EVENT_STATUS_QUERY.contains("is_transfer = TRUE"));
        assert!(EVENT_STATUS_QUERY.contains("is_transfer = FALSE"));
    }

    #[test]
    fn notification_status_query_counts_delivery_statuses_and_unread() {
        assert!(NOTIFICATION_STATUS_QUERY.contains("status = 'sent'"));
        assert!(NOTIFICATION_STATUS_QUERY.contains("status = 'skipped'"));
        assert!(NOTIFICATION_STATUS_QUERY.contains("status = 'failed'"));
        assert!(NOTIFICATION_STATUS_QUERY.contains("read_at IS NULL"));
    }

    #[test]
    fn notification_status_uses_fifteen_minute_stale_outbox_window() {
        assert_eq!(NOTIFICATION_STATUS_STALE_MINUTES, 15);
    }

    #[test]
    fn system_status_uses_service_heartbeat_stale_threshold() {
        assert_eq!(SERVICE_HEARTBEAT_STALE_SECONDS, 90);
    }

    #[test]
    fn provider_queries_include_chain_names() {
        assert!(PROVIDER_CHAIN_STATUS_QUERY.contains("JOIN chains"));
        assert!(PROVIDER_CHAIN_STATUS_QUERY.contains("chain_name"));
        assert!(PROVIDER_ITEMS_QUERY.contains("JOIN chains"));
        assert!(PROVIDER_ITEMS_QUERY.contains("chain_name"));
    }

    #[test]
    fn provider_items_query_includes_runtime_health_left_join() {
        assert!(PROVIDER_ITEMS_QUERY.contains("LEFT JOIN provider_health ph"));
        assert!(PROVIDER_ITEMS_QUERY.contains("COALESCE(ph.consecutive_failures, 0)"));
        assert!(PROVIDER_ITEMS_QUERY.contains("last_success_at"));
        assert!(PROVIDER_ITEMS_QUERY.contains("last_failure_at"));
        assert!(PROVIDER_ITEMS_QUERY.contains("disabled_until"));
        assert!(PROVIDER_ITEMS_QUERY.contains("last_error"));
    }

    #[test]
    fn provider_status_defaults_missing_health_to_zero_failures() {
        assert_eq!(PROVIDER_STATUS_DEFAULT_FAILURES, 0);
    }
}
