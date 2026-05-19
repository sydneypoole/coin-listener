use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub display_name: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Tenant {
    pub id: Uuid,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Chain {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub chain_type: String,
    pub native_asset_symbol: String,
    pub status: String,
    pub default_confirmations: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Provider {
    pub id: Uuid,
    pub chain_id: Uuid,
    pub provider_type: String,
    pub name: String,
    pub base_url: String,
    pub api_key_ref: Option<String>,
    pub priority: i32,
    pub qps_limit: i32,
    pub timeout_ms: i32,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Asset {
    pub id: Uuid,
    pub chain_id: Uuid,
    pub asset_type: String,
    pub symbol: String,
    pub name: String,
    pub contract_address: Option<String>,
    pub decimals: i32,
    pub is_builtin: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WatchedAddress {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address: String,
    pub label: Option<String>,
    pub priority: String,
    pub scan_interval_seconds: i32,
    pub transfer_filter_enabled: bool,
    pub balance_change_filter_enabled: bool,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: UserSummary,
    pub tenant: Tenant,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserSummary {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProviderRequest {
    pub chain_id: Uuid,
    pub provider_type: String,
    pub name: String,
    pub base_url: String,
    pub api_key_ref: Option<String>,
    pub priority: i32,
    pub qps_limit: i32,
    pub timeout_ms: i32,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateWatchedAddressRequest {
    pub tenant_id: Option<Uuid>,
    pub chain_id: Uuid,
    pub address: String,
    pub label: Option<String>,
    pub priority: String,
    pub scan_interval_seconds: i32,
    pub transfer_filter_enabled: bool,
    pub balance_change_filter_enabled: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct BalanceSnapshot {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address_id: Uuid,
    pub asset_id: Uuid,
    pub balance_raw: String,
    pub balance_decimal: String,
    pub block_number: Option<i64>,
    pub block_hash: Option<String>,
    pub observed_at: DateTime<Utc>,
    pub source_provider_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateBalanceSnapshotRequest {
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address_id: Uuid,
    pub asset_id: Uuid,
    pub balance_raw: String,
    pub balance_decimal: String,
    pub block_number: Option<i64>,
    pub block_hash: Option<String>,
    pub source_provider_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AddressEvent {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address_id: Uuid,
    pub asset_id: Uuid,
    pub event_type: String,
    pub direction: String,
    pub is_transfer: bool,
    pub tx_hash: Option<String>,
    pub log_index: Option<i32>,
    pub block_number: Option<i64>,
    pub block_hash: Option<String>,
    pub confirmations: i32,
    pub from_address: Option<String>,
    pub to_address: Option<String>,
    pub amount_raw: Option<String>,
    pub amount_decimal: Option<String>,
    pub balance_before_raw: Option<String>,
    pub balance_after_raw: Option<String>,
    pub balance_delta_raw: Option<String>,
    pub metadata: serde_json::Value,
    pub detected_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventQuery {
    pub chain_id: Option<Uuid>,
    pub address_id: Option<Uuid>,
    pub asset_id: Option<Uuid>,
    pub event_type: Option<String>,
    pub direction: Option<String>,
    pub is_transfer: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemStatus {
    pub generated_at: DateTime<Utc>,
    pub queues: QueueStatus,
    pub scans: ScanStatus,
    pub events: EventStatus,
    pub notifications: NotificationStatus,
    pub providers: ProviderStatus,
    pub services: ServiceHealthStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueStatus {
    pub scan_queue_key: String,
    pub scan_queue_depth: Option<i64>,
    pub notify_queue_key: String,
    pub notify_queue_depth: Option<i64>,
    pub queue_errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanStatus {
    pub active_addresses: i64,
    pub due_addresses: i64,
    pub overdue_addresses: i64,
    pub last_scanned_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventStatus {
    pub last_24h_total: i64,
    pub last_24h_transfers: i64,
    pub last_24h_non_transfers: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationStatus {
    pub last_24h_sent: i64,
    pub last_24h_skipped: i64,
    pub last_24h_failed: i64,
    pub unread_in_app: i64,
    pub outbox: OutboxStatusCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct OutboxStatusCounts {
    pub pending: i64,
    pub retryable: i64,
    pub processing: i64,
    pub failed: i64,
    pub stale_processing: i64,
    pub next_due_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceHeartbeatStatusItem {
    pub service_name: String,
    pub instance_id: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub stale_after_seconds: i64,
    pub is_stale: bool,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceHealthStatus {
    pub online: i64,
    pub stale: i64,
    pub items: Vec<ServiceHeartbeatStatusItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub active: i64,
    pub inactive: i64,
    pub by_chain: Vec<ProviderChainStatus>,
    pub items: Vec<ProviderStatusItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderChainStatus {
    pub chain_id: Uuid,
    pub chain_name: String,
    pub active: i64,
    pub inactive: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatusItem {
    pub id: Uuid,
    pub chain_id: Uuid,
    pub chain_name: String,
    pub provider_type: String,
    pub name: String,
    pub base_url: String,
    pub priority: i32,
    pub qps_limit: i32,
    pub timeout_ms: i32,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanAddressTask {
    pub task_id: Uuid,
    pub address_id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub attempt: u16,
    pub enqueued_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyEventTask {
    pub task_id: Uuid,
    pub event_id: Uuid,
    pub tenant_id: Uuid,
    pub attempt: u16,
    pub enqueued_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationChannel {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub channel_type: String,
    pub name: String,
    pub config: serde_json::Value,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateNotificationChannelRequest {
    pub channel_type: String,
    pub name: String,
    pub config: Option<serde_json::Value>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationRule {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub chain_id: Option<Uuid>,
    pub address_id: Option<Uuid>,
    pub asset_id: Option<Uuid>,
    pub event_type: Option<String>,
    pub is_transfer: Option<bool>,
    pub min_amount_raw: Option<String>,
    pub direction: Option<String>,
    pub channel_ids: Vec<Uuid>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateNotificationRuleRequest {
    pub name: String,
    pub chain_id: Option<Uuid>,
    pub address_id: Option<Uuid>,
    pub asset_id: Option<Uuid>,
    pub event_type: Option<String>,
    pub is_transfer: Option<bool>,
    pub min_amount_raw: Option<String>,
    pub direction: Option<String>,
    pub channel_ids: Option<Vec<Uuid>>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationDelivery {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub rule_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub status: String,
    pub attempt_count: i32,
    pub last_error: Option<String>,
    pub sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub channel_type: Option<String>,
    pub idempotency_key: Option<String>,
    pub provider_message_id: Option<String>,
    pub provider_status_code: Option<i32>,
    pub provider_response: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct InAppNotification {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub delivery_id: Option<Uuid>,
    pub title: String,
    pub body: String,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotificationDeliveryQuery {
    pub event_id: Option<Uuid>,
    pub status: Option<String>,
    pub channel_type: Option<String>,
    pub rule_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationDeliveryListItem {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub rule_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub channel_type: Option<String>,
    pub status: String,
    pub attempt_count: i32,
    pub last_error: Option<String>,
    pub sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub idempotency_key: Option<String>,
    pub provider_message_id: Option<String>,
    pub provider_status_code: Option<i32>,
    pub provider_response: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDeliveryListResponse {
    pub items: Vec<NotificationDeliveryListItem>,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationOutboxItem {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub status: String,
    pub attempt_count: i32,
    pub next_attempt_at: DateTime<Utc>,
    pub locked_at: Option<DateTime<Utc>>,
    pub locked_by: Option<String>,
    pub last_error: Option<String>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NotificationOutboxQuery {
    pub status: Option<String>,
    pub event_id: Option<Uuid>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationOutboxListItem {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_id: Uuid,
    pub status: String,
    pub attempt_count: i32,
    pub next_attempt_at: DateTime<Utc>,
    pub locked_at: Option<DateTime<Utc>>,
    pub locked_by: Option<String>,
    pub last_error: Option<String>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub event_type: Option<String>,
    pub direction: Option<String>,
    pub tx_hash: Option<String>,
    pub delivery_total: i64,
    pub delivery_sent: i64,
    pub delivery_failed: i64,
    pub delivery_skipped: i64,
    pub is_stale_processing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationOutboxListResponse {
    pub items: Vec<NotificationOutboxListItem>,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationOutboxDetail {
    pub outbox: NotificationOutboxListItem,
    pub event: AddressEvent,
    pub deliveries: Vec<NotificationDeliveryListItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryNotificationOutboxResponse {
    pub outbox: NotificationOutboxItem,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InAppNotificationQuery {
    pub unread_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScanAddressCandidate {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub scan_interval_seconds: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScanAddressContext {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address: String,
    pub scan_interval_seconds: i32,
    pub chain_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ScanCursor {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address_id: Uuid,
    pub cursor_type: String,
    pub last_scanned_block: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AddressEventDraft {
    pub tenant_id: Uuid,
    pub chain_id: Uuid,
    pub address_id: Uuid,
    pub asset_id: Uuid,
    pub event_type: String,
    pub direction: String,
    pub is_transfer: bool,
    pub tx_hash: Option<String>,
    pub log_index: Option<i32>,
    pub block_number: Option<i64>,
    pub block_hash: Option<String>,
    pub confirmations: i32,
    pub from_address: Option<String>,
    pub to_address: Option<String>,
    pub amount_raw: Option<String>,
    pub amount_decimal: Option<String>,
    pub balance_before_raw: Option<String>,
    pub balance_after_raw: Option<String>,
    pub balance_delta_raw: Option<String>,
    pub metadata: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::{
        AddressEvent, CreateBalanceSnapshotRequest, EventStatus, NotificationDelivery,
        NotificationDeliveryListItem, NotificationDeliveryListResponse, NotificationDeliveryQuery,
        NotificationOutboxDetail, NotificationOutboxItem, NotificationOutboxListItem,
        NotificationOutboxListResponse, NotificationOutboxQuery, NotificationStatus,
        NotifyEventTask, OutboxStatusCounts, ProviderChainStatus, ProviderStatus,
        ProviderStatusItem, QueueStatus, RetryNotificationOutboxResponse, ScanAddressTask,
        ScanCursor, ScanStatus, ServiceHealthStatus, ServiceHeartbeatStatusItem, SystemStatus,
    };
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    #[test]
    fn service_health_status_round_trips_as_json() {
        let status = ServiceHealthStatus {
            online: 1,
            stale: 1,
            items: vec![ServiceHeartbeatStatusItem {
                service_name: "worker".to_string(),
                instance_id: "instance-1".to_string(),
                status: "online".to_string(),
                started_at: Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap(),
                last_seen_at: Utc.with_ymd_and_hms(2026, 5, 19, 10, 1, 0).unwrap(),
                stale_after_seconds: 90,
                is_stale: false,
                metadata: serde_json::json!({ "pid": 1234, "version": "0.1.0" }),
            }],
        };

        let payload = serde_json::to_string(&status).expect("serialize service health");
        let decoded: ServiceHealthStatus =
            serde_json::from_str(&payload).expect("deserialize service health");

        assert_eq!(decoded, status);
        assert!(payload.contains("\"service_name\":\"worker\""));
        assert!(payload.contains("\"stale_after_seconds\":90"));
    }

    #[test]
    fn system_status_round_trips_as_json() {
        let status = SystemStatus {
            generated_at: Utc.with_ymd_and_hms(2026, 5, 17, 10, 0, 0).unwrap(),
            queues: QueueStatus {
                scan_queue_key: "scan:address:queue".to_string(),
                scan_queue_depth: Some(3),
                notify_queue_key: "notify:event:queue".to_string(),
                notify_queue_depth: Some(1),
                queue_errors: vec![],
            },
            scans: ScanStatus {
                active_addresses: 12,
                due_addresses: 2,
                overdue_addresses: 1,
                last_scanned_at: Some(Utc.with_ymd_and_hms(2026, 5, 17, 9, 58, 0).unwrap()),
            },
            events: EventStatus {
                last_24h_total: 40,
                last_24h_transfers: 35,
                last_24h_non_transfers: 5,
            },
            notifications: NotificationStatus {
                last_24h_sent: 20,
                last_24h_skipped: 2,
                last_24h_failed: 1,
                unread_in_app: 4,
                outbox: OutboxStatusCounts {
                    pending: 0,
                    retryable: 0,
                    processing: 0,
                    failed: 0,
                    stale_processing: 0,
                    next_due_at: None,
                },
            },
            providers: ProviderStatus {
                active: 4,
                inactive: 1,
                by_chain: vec![ProviderChainStatus {
                    chain_id: Uuid::from_u128(1),
                    chain_name: "Ethereum".to_string(),
                    active: 2,
                    inactive: 0,
                }],
                items: vec![ProviderStatusItem {
                    id: Uuid::from_u128(10),
                    chain_id: Uuid::from_u128(1),
                    chain_name: "Ethereum".to_string(),
                    provider_type: "rpc".to_string(),
                    name: "Ethereum RPC".to_string(),
                    base_url: "https://example.invalid".to_string(),
                    priority: 1,
                    qps_limit: 10,
                    timeout_ms: 5000,
                    status: "active".to_string(),
                }],
            },
            services: ServiceHealthStatus {
                online: 1,
                stale: 0,
                items: vec![ServiceHeartbeatStatusItem {
                    service_name: "api-server".to_string(),
                    instance_id: "api-1".to_string(),
                    status: "online".to_string(),
                    started_at: Utc.with_ymd_and_hms(2026, 5, 17, 10, 0, 0).unwrap(),
                    last_seen_at: Utc.with_ymd_and_hms(2026, 5, 17, 10, 0, 30).unwrap(),
                    stale_after_seconds: 90,
                    is_stale: false,
                    metadata: serde_json::json!({ "pid": 100, "version": "0.1.0" }),
                }],
            },
        };

        let payload = serde_json::to_string(&status).expect("serialize system status");
        let decoded: SystemStatus =
            serde_json::from_str(&payload).expect("deserialize system status");

        assert_eq!(decoded, status);
        assert!(payload.contains("\"scan_queue_depth\":3"));
        assert!(payload.contains("\"unread_in_app\":4"));
        assert!(payload.contains("\"services\""));
    }

    #[test]
    fn queue_status_allows_missing_depths_with_errors() {
        let queues = QueueStatus {
            scan_queue_key: "scan:address:queue".to_string(),
            scan_queue_depth: None,
            notify_queue_key: "notify:event:queue".to_string(),
            notify_queue_depth: None,
            queue_errors: vec!["redis unavailable".to_string()],
        };

        let payload = serde_json::to_string(&queues).expect("serialize queue status");
        let decoded: QueueStatus =
            serde_json::from_str(&payload).expect("deserialize queue status");

        assert_eq!(decoded.scan_queue_depth, None);
        assert_eq!(decoded.notify_queue_depth, None);
        assert_eq!(decoded.queue_errors, vec!["redis unavailable"]);
        assert!(payload.contains("\"scan_queue_depth\":null"));
    }

    #[test]
    fn notification_delivery_round_trips_external_metadata() {
        let delivery = NotificationDelivery {
            id: Uuid::from_u128(11),
            tenant_id: Uuid::from_u128(12),
            event_id: Uuid::from_u128(13),
            rule_id: Some(Uuid::from_u128(14)),
            channel_id: Some(Uuid::from_u128(15)),
            status: "sent".to_string(),
            attempt_count: 1,
            last_error: None,
            sent_at: Some(Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap()),
            created_at: Utc.with_ymd_and_hms(2026, 5, 18, 11, 59, 0).unwrap(),
            channel_type: Some("telegram".to_string()),
            idempotency_key: Some("event-rule-channel".to_string()),
            provider_message_id: Some("message-123".to_string()),
            provider_status_code: Some(200),
            provider_response: Some("{\"ok\":true}".to_string()),
        };

        let payload = serde_json::to_string(&delivery).expect("serialize notification delivery");
        let decoded: NotificationDelivery =
            serde_json::from_str(&payload).expect("deserialize notification delivery");

        assert_eq!(decoded.channel_type.as_deref(), Some("telegram"));
        assert_eq!(
            decoded.idempotency_key.as_deref(),
            Some("event-rule-channel")
        );
        assert_eq!(decoded.provider_message_id.as_deref(), Some("message-123"));
        assert_eq!(decoded.provider_status_code, Some(200));
        assert_eq!(decoded.provider_response.as_deref(), Some("{\"ok\":true}"));
    }

    #[test]
    fn notification_outbox_item_round_trips_as_json() {
        let item = NotificationOutboxItem {
            id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            event_id: Uuid::from_u128(3),
            status: "processing".to_string(),
            attempt_count: 2,
            next_attempt_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap(),
            locked_at: Some(Utc.with_ymd_and_hms(2026, 5, 18, 12, 1, 0).unwrap()),
            locked_by: Some("notifier-test".to_string()),
            last_error: Some("temporary failure".to_string()),
            delivered_at: None,
            created_at: Utc.with_ymd_and_hms(2026, 5, 18, 11, 59, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 1, 0).unwrap(),
        };

        let payload = serde_json::to_string(&item).expect("serialize notification outbox item");
        let decoded: NotificationOutboxItem =
            serde_json::from_str(&payload).expect("deserialize notification outbox item");

        assert_eq!(decoded.id, item.id);
        assert_eq!(decoded.tenant_id, item.tenant_id);
        assert_eq!(decoded.event_id, item.event_id);
        assert_eq!(decoded.status, "processing");
        assert_eq!(decoded.attempt_count, 2);
        assert_eq!(decoded.locked_by.as_deref(), Some("notifier-test"));
        assert!(payload.contains("\"last_error\":\"temporary failure\""));
    }

    #[test]
    fn notification_status_round_trips_outbox_counts() {
        let status = NotificationStatus {
            last_24h_sent: 20,
            last_24h_skipped: 2,
            last_24h_failed: 1,
            unread_in_app: 4,
            outbox: OutboxStatusCounts {
                pending: 3,
                retryable: 2,
                processing: 1,
                failed: 5,
                stale_processing: 1,
                next_due_at: Some(Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap()),
            },
        };

        let payload = serde_json::to_string(&status).expect("serialize notification status");
        let decoded: NotificationStatus =
            serde_json::from_str(&payload).expect("deserialize notification status");

        assert_eq!(decoded, status);
        assert!(payload.contains("\"pending\":3"));
        assert!(payload.contains("\"stale_processing\":1"));
    }

    #[test]
    fn notification_operations_queries_deserialize_filters() {
        let outbox_query: NotificationOutboxQuery = serde_json::from_str(
            r#"{"status":"failed","event_id":"00000000-0000-0000-0000-000000000001","limit":50,"offset":10}"#,
        )
        .expect("deserialize outbox query");
        assert_eq!(outbox_query.status.as_deref(), Some("failed"));
        assert_eq!(outbox_query.limit, Some(50));
        assert_eq!(outbox_query.offset, Some(10));

        let delivery_query: NotificationDeliveryQuery = serde_json::from_str(
            r#"{"status":"failed","channel_type":"webhook","rule_id":"00000000-0000-0000-0000-000000000002","channel_id":"00000000-0000-0000-0000-000000000003","limit":25,"offset":5}"#,
        )
        .expect("deserialize delivery query");
        assert_eq!(delivery_query.status.as_deref(), Some("failed"));
        assert_eq!(delivery_query.channel_type.as_deref(), Some("webhook"));
        assert_eq!(delivery_query.limit, Some(25));
        assert_eq!(delivery_query.offset, Some(5));
    }

    #[test]
    fn notification_operations_responses_round_trip_provider_metadata() {
        let created_at = Utc.with_ymd_and_hms(2026, 5, 19, 9, 0, 0).unwrap();
        let event_id = Uuid::from_u128(13);
        let outbox = NotificationOutboxListItem {
            id: Uuid::from_u128(11),
            tenant_id: Uuid::from_u128(12),
            event_id,
            status: "failed".to_string(),
            attempt_count: 5,
            next_attempt_at: Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap(),
            locked_at: None,
            locked_by: None,
            last_error: Some("webhook returned retryable status 500".to_string()),
            delivered_at: None,
            created_at,
            updated_at: created_at,
            event_type: Some("transfer".to_string()),
            direction: Some("in".to_string()),
            tx_hash: Some("0xabc".to_string()),
            delivery_total: 2,
            delivery_sent: 1,
            delivery_failed: 1,
            delivery_skipped: 0,
            is_stale_processing: false,
        };
        let delivery = NotificationDeliveryListItem {
            id: Uuid::from_u128(21),
            tenant_id: Uuid::from_u128(12),
            event_id,
            rule_id: Some(Uuid::from_u128(22)),
            channel_id: Some(Uuid::from_u128(23)),
            channel_type: Some("webhook".to_string()),
            status: "failed".to_string(),
            attempt_count: 3,
            last_error: Some("webhook returned retryable status 500".to_string()),
            sent_at: None,
            created_at,
            idempotency_key: Some("notification:v1:tenant:event:rule:channel".to_string()),
            provider_message_id: None,
            provider_status_code: Some(500),
            provider_response: Some("server error".to_string()),
        };
        let event = AddressEvent {
            id: event_id,
            tenant_id: Uuid::from_u128(12),
            chain_id: Uuid::from_u128(31),
            address_id: Uuid::from_u128(32),
            asset_id: Uuid::from_u128(33),
            event_type: "transfer".to_string(),
            direction: "in".to_string(),
            is_transfer: true,
            tx_hash: Some("0xabc".to_string()),
            log_index: Some(1),
            block_number: Some(100),
            block_hash: Some("0xblock".to_string()),
            confirmations: 12,
            from_address: Some("0xfrom".to_string()),
            to_address: Some("0xto".to_string()),
            amount_raw: Some("1000".to_string()),
            amount_decimal: Some("0.001".to_string()),
            balance_before_raw: None,
            balance_after_raw: None,
            balance_delta_raw: None,
            metadata: serde_json::json!({"source":"test"}),
            detected_at: created_at,
            created_at,
        };
        let detail = NotificationOutboxDetail {
            outbox: outbox.clone(),
            event,
            deliveries: vec![delivery.clone()],
        };

        let outbox_list = NotificationOutboxListResponse {
            items: vec![outbox.clone()],
            limit: 50,
            offset: 0,
        };
        let delivery_list = NotificationDeliveryListResponse {
            items: vec![delivery],
            limit: 50,
            offset: 0,
        };
        let retry = RetryNotificationOutboxResponse {
            outbox: NotificationOutboxItem {
                id: outbox.id,
                tenant_id: outbox.tenant_id,
                event_id: outbox.event_id,
                status: "retryable".to_string(),
                attempt_count: outbox.attempt_count,
                next_attempt_at: outbox.next_attempt_at,
                locked_at: None,
                locked_by: None,
                last_error: None,
                delivered_at: None,
                created_at: outbox.created_at,
                updated_at: outbox.updated_at,
            },
        };

        let payload = serde_json::to_string(&(outbox_list, detail, delivery_list, retry))
            .expect("serialize operations responses");

        assert!(payload.contains("\"delivery_failed\":1"));
        assert!(payload.contains("\"provider_status_code\":500"));
        assert!(payload.contains("\"outbox\""));
    }

    #[test]
    fn create_balance_snapshot_request_round_trips_as_json() {
        let request = CreateBalanceSnapshotRequest {
            tenant_id: Uuid::from_u128(101),
            chain_id: Uuid::from_u128(102),
            address_id: Uuid::from_u128(103),
            asset_id: Uuid::from_u128(104),
            balance_raw: "1000000000000000000".to_string(),
            balance_decimal: "1.0".to_string(),
            block_number: Some(20_000_000),
            block_hash: None,
            source_provider_id: Some(Uuid::from_u128(105)),
        };

        let payload = serde_json::to_string(&request).expect("serialize snapshot request");
        let decoded: CreateBalanceSnapshotRequest =
            serde_json::from_str(&payload).expect("deserialize snapshot request");

        assert_eq!(decoded, request);
        assert!(payload.contains("\"balance_raw\":\"1000000000000000000\""));
        assert!(payload.contains("\"block_number\":20000000"));
    }

    #[test]
    fn notify_event_task_round_trips_as_json() {
        let task = NotifyEventTask {
            task_id: Uuid::from_u128(11),
            event_id: Uuid::from_u128(12),
            tenant_id: Uuid::from_u128(13),
            attempt: 1,
            enqueued_at: Utc.with_ymd_and_hms(2026, 5, 17, 15, 0, 0).unwrap(),
        };

        let payload = serde_json::to_string(&task).expect("serialize notify task");
        let decoded: NotifyEventTask =
            serde_json::from_str(&payload).expect("deserialize notify task");

        assert_eq!(decoded, task);
        assert!(payload.contains("\"attempt\":1"));
    }

    #[test]
    fn scan_address_task_round_trips_as_json() {
        let task = ScanAddressTask {
            task_id: Uuid::from_u128(1),
            address_id: Uuid::from_u128(2),
            tenant_id: Uuid::from_u128(3),
            chain_id: Uuid::from_u128(4),
            attempt: 1,
            enqueued_at: Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 0).unwrap(),
        };

        let payload = serde_json::to_string(&task).expect("serialize scan task");
        let decoded: ScanAddressTask =
            serde_json::from_str(&payload).expect("deserialize scan task");

        assert_eq!(decoded, task);
        assert!(payload.contains("\"attempt\":1"));
    }

    #[test]
    fn scan_cursor_round_trips_as_json() {
        let cursor = ScanCursor {
            id: Uuid::from_u128(21),
            tenant_id: Uuid::from_u128(22),
            chain_id: Uuid::from_u128(23),
            address_id: Uuid::from_u128(24),
            cursor_type: "evm_erc20_transfer".to_string(),
            last_scanned_block: 20_000_000,
            updated_at: Utc.with_ymd_and_hms(2026, 5, 17, 22, 0, 0).unwrap(),
        };

        let payload = serde_json::to_string(&cursor).expect("serialize scan cursor");
        let decoded: ScanCursor = serde_json::from_str(&payload).expect("deserialize scan cursor");

        assert_eq!(decoded.id, cursor.id);
        assert_eq!(decoded.cursor_type, "evm_erc20_transfer");
        assert_eq!(decoded.last_scanned_block, 20_000_000);
    }
}
