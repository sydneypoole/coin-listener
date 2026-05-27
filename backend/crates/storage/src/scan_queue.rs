use coin_listener_core::{models::ScanAddressTask, AppError, AppResult};
use redis::{aio::MultiplexedConnection, Client};

const SCAN_WAKE_SIGNAL_PAYLOAD: &str = "wake";

#[derive(Debug, Clone)]
pub struct ScanQueue {
    queue_key: String,
    lock_ttl_seconds: u64,
}

impl ScanQueue {
    pub fn new(queue_key: String, lock_ttl_seconds: u64) -> Self {
        Self {
            queue_key,
            lock_ttl_seconds,
        }
    }

    pub fn queue_key(&self) -> &str {
        &self.queue_key
    }

    pub fn lock_ttl_seconds(&self) -> u64 {
        self.lock_ttl_seconds
    }

    pub async fn depth(&self, connection: &mut MultiplexedConnection) -> AppResult<i64> {
        let depth: i64 = redis::cmd("LLEN")
            .arg(&self.queue_key)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;
        Ok(depth)
    }

    pub async fn signal(&self, connection: &mut MultiplexedConnection) -> AppResult<()> {
        let _: () = redis::Script::new(
            r#"
            local values = redis.call('LRANGE', KEYS[1], 0, -1)
            local has_wake = false
            for _, value in ipairs(values) do
                if value == ARGV[1] then
                    has_wake = true
                    break
                end
            end
            if has_wake then
                return
            end
            redis.call('LPUSH', KEYS[1], ARGV[1])
            "#,
        )
        .key(&self.queue_key)
        .arg(SCAN_WAKE_SIGNAL_PAYLOAD)
        .invoke_async(connection)
        .await
        .map_err(|error| AppError::Redis(error.to_string()))?;
        Ok(())
    }

    pub async fn wait_for_signal(
        &self,
        connection: &mut MultiplexedConnection,
        timeout_seconds: usize,
    ) -> AppResult<bool> {
        let result: Option<(String, String)> = redis::cmd("BRPOP")
            .arg(&self.queue_key)
            .arg(timeout_seconds)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;

        let Some((_, payload)) = result else {
            return Ok(false);
        };
        if payload == SCAN_WAKE_SIGNAL_PAYLOAD {
            return Ok(true);
        }
        if deserialize_scan_task(&payload).is_ok() {
            return Err(AppError::Redis(
                "legacy Redis scan task payload found in wakeup queue".to_string(),
            ));
        }
        Ok(true)
    }

    pub async fn enqueue(
        &self,
        connection: &mut MultiplexedConnection,
        _task: &ScanAddressTask,
    ) -> AppResult<()> {
        self.signal(connection).await
    }
}

pub async fn connect_scan_queue(client: &Client) -> AppResult<MultiplexedConnection> {
    client
        .get_multiplexed_async_connection()
        .await
        .map_err(|error| AppError::Redis(error.to_string()))
}

pub fn queue_depth_command(queue_key: &str) -> [&str; 2] {
    ["LLEN", queue_key]
}

pub fn wake_signal_payload() -> &'static str {
    SCAN_WAKE_SIGNAL_PAYLOAD
}

pub fn serialize_scan_task(task: &ScanAddressTask) -> AppResult<String> {
    serde_json::to_string(task).map_err(|error| AppError::Validation(error.to_string()))
}

pub fn deserialize_scan_task(payload: &str) -> AppResult<ScanAddressTask> {
    serde_json::from_str(payload).map_err(|error| AppError::Validation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        deserialize_scan_task, queue_depth_command, serialize_scan_task, wake_signal_payload,
        ScanQueue,
    };
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::ScanAddressTask;
    use uuid::Uuid;

    #[test]
    fn scan_task_payload_round_trips_for_legacy_compatibility() {
        let task = ScanAddressTask {
            task_id: Uuid::from_u128(11),
            address_id: Uuid::from_u128(12),
            tenant_id: Uuid::from_u128(13),
            chain_id: Uuid::from_u128(14),
            attempt: 1,
            enqueued_at: Utc.with_ymd_and_hms(2026, 5, 17, 13, 0, 0).unwrap(),
        };

        let payload = serialize_scan_task(&task).expect("serialize task");
        let decoded = deserialize_scan_task(&payload).expect("deserialize task");

        assert_eq!(decoded, task);
    }

    #[test]
    fn malformed_scan_task_payload_returns_error() {
        let result = deserialize_scan_task("not-json");

        assert!(result.is_err());
    }

    #[test]
    fn scan_queue_keeps_key_and_lock_ttl_for_runtime_wiring() {
        let queue = ScanQueue::new("scan:address:queue".to_string(), 120);

        assert_eq!(queue.queue_key(), "scan:address:queue");
        assert_eq!(queue.lock_ttl_seconds(), 120);
    }

    #[test]
    fn scan_wakeup_payload_is_disposable_signal_not_task_json() {
        assert_eq!(wake_signal_payload(), "wake");
        assert!(deserialize_scan_task(wake_signal_payload()).is_err());
    }

    #[test]
    fn scan_wakeup_signal_is_coalesced_to_one_token() {
        let source = include_str!("scan_queue.rs");
        let signal_index = source.find("pub async fn signal(").unwrap();
        let wait_index = source.find("pub async fn wait_for_signal").unwrap();
        let signal_source = &source[signal_index..wait_index];

        assert!(signal_source.contains("redis::Script"));
        assert!(signal_source.contains("redis.call('LRANGE'"));
        assert!(signal_source.contains("has_wake"));
        assert!(signal_source.contains("redis.call('LPUSH'"));
    }

    #[test]
    fn scan_wakeup_signal_does_not_trim_legacy_payloads_before_detection() {
        let source = include_str!("scan_queue.rs");
        let signal_index = source.find("pub async fn signal(").unwrap();
        let wait_index = source.find("pub async fn wait_for_signal").unwrap();
        let signal_source = &source[signal_index..wait_index];

        assert!(signal_source.contains("SCAN_WAKE_SIGNAL_PAYLOAD"));
        assert!(signal_source.contains("redis.call('LRANGE'"));
        assert!(signal_source.contains("redis.call('LPUSH'"));
        assert!(!signal_source.contains("redis.call('LTRIM'"));
        assert!(signal_source.find("redis.call('LRANGE'").unwrap() < signal_source.find("redis.call('LPUSH'").unwrap());
    }

    #[test]
    fn scan_wakeup_signal_remains_bounded_with_legacy_payloads() {
        let source = include_str!("scan_queue.rs");
        let signal_index = source.find("pub async fn signal(").unwrap();
        let wait_index = source.find("pub async fn wait_for_signal").unwrap();
        let signal_source = &source[signal_index..wait_index];

        assert!(signal_source.contains("redis.call('LRANGE'"));
        assert!(signal_source.contains("has_wake"));
        assert!(signal_source.contains("if has_wake then"));
        assert!(signal_source.find("has_wake").unwrap() < signal_source.rfind("redis.call('LPUSH'").unwrap());
    }

    #[test]
    fn legacy_json_scan_payload_is_rejected_instead_of_silently_dropped() {
        let source = include_str!("scan_queue.rs");
        let wait_index = source.find("pub async fn wait_for_signal").unwrap();
        let enqueue_index = source.find("pub async fn enqueue").unwrap();
        let wait_source = &source[wait_index..enqueue_index];

        assert!(wait_source.contains("deserialize_scan_task(&payload)"));
        assert!(wait_source.contains("legacy Redis scan task payload"));
    }

    #[test]
    fn queue_depth_command_uses_llen_and_queue_key() {
        assert_eq!(
            queue_depth_command("scan:address:queue"),
            ["LLEN", "scan:address:queue"]
        );
    }
}
