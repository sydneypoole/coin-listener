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
        CreateBalanceSnapshotRequest, EventStatus, NotificationStatus, NotifyEventTask,
        ProviderChainStatus, ProviderStatus, ProviderStatusItem, QueueStatus, ScanAddressTask,
        ScanCursor, ScanStatus, SystemStatus,
    };
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

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
        };

        let payload = serde_json::to_string(&status).expect("serialize system status");
        let decoded: SystemStatus =
            serde_json::from_str(&payload).expect("deserialize system status");

        assert_eq!(decoded, status);
        assert!(payload.contains("\"scan_queue_depth\":3"));
        assert!(payload.contains("\"unread_in_app\":4"));
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
