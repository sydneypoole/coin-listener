use coin_listener_core::{models::ScanAddressTask, AppError, AppResult};
use redis::{aio::MultiplexedConnection, Client};
use uuid::Uuid;

const LOCK_KEY_PREFIX: &str = "scan:address:lock";
const RELEASE_LOCK_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) == ARGV[1] then
    return redis.call("DEL", KEYS[1])
else
    return 0
end
"#;

#[derive(Debug, Clone)]
pub struct ScanQueue {
    queue_key: String,
    lock_ttl_seconds: usize,
}

impl ScanQueue {
    pub fn new(queue_key: String, lock_ttl_seconds: usize) -> Self {
        Self {
            queue_key,
            lock_ttl_seconds,
        }
    }

    pub fn queue_key(&self) -> &str {
        &self.queue_key
    }

    pub async fn depth(&self, connection: &mut MultiplexedConnection) -> AppResult<i64> {
        let depth: i64 = redis::cmd("LLEN")
            .arg(&self.queue_key)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;
        Ok(depth)
    }

    pub async fn enqueue(
        &self,
        connection: &mut MultiplexedConnection,
        task: &ScanAddressTask,
    ) -> AppResult<()> {
        let payload = serialize_scan_task(task)?;
        let _: usize = redis::cmd("LPUSH")
            .arg(&self.queue_key)
            .arg(payload)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;
        Ok(())
    }

    pub async fn dequeue(
        &self,
        connection: &mut MultiplexedConnection,
        timeout_seconds: usize,
    ) -> AppResult<Option<ScanAddressTask>> {
        let result: Option<(String, String)> = redis::cmd("BRPOP")
            .arg(&self.queue_key)
            .arg(timeout_seconds)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;

        result
            .map(|(_, payload)| deserialize_scan_task(&payload))
            .transpose()
    }

    pub async fn acquire_lock(
        &self,
        connection: &mut MultiplexedConnection,
        address_id: Uuid,
        task_id: Uuid,
    ) -> AppResult<bool> {
        let result: Option<String> = redis::cmd("SET")
            .arg(scan_lock_key(address_id))
            .arg(task_id.to_string())
            .arg("NX")
            .arg("EX")
            .arg(self.lock_ttl_seconds)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;

        Ok(result.is_some())
    }

    pub async fn release_lock(
        &self,
        connection: &mut MultiplexedConnection,
        address_id: Uuid,
        task_id: Uuid,
    ) -> AppResult<bool> {
        let released: i32 = redis::Script::new(RELEASE_LOCK_SCRIPT)
            .key(scan_lock_key(address_id))
            .arg(task_id.to_string())
            .invoke_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;

        Ok(released > 0)
    }
}

pub async fn connect_scan_queue(client: &Client) -> AppResult<MultiplexedConnection> {
    client
        .get_multiplexed_async_connection()
        .await
        .map_err(|error| AppError::Redis(error.to_string()))
}

pub fn scan_lock_key(address_id: Uuid) -> String {
    format!("{LOCK_KEY_PREFIX}:{address_id}")
}

pub fn queue_depth_command(queue_key: &str) -> [&str; 2] {
    ["LLEN", queue_key]
}

pub fn serialize_scan_task(task: &ScanAddressTask) -> AppResult<String> {
    serde_json::to_string(task).map_err(|error| AppError::Validation(error.to_string()))
}

pub fn deserialize_scan_task(payload: &str) -> AppResult<ScanAddressTask> {
    serde_json::from_str(payload).map_err(|error| AppError::Validation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{deserialize_scan_task, queue_depth_command, scan_lock_key, serialize_scan_task};
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::ScanAddressTask;
    use uuid::Uuid;

    #[test]
    fn scan_task_payload_round_trips() {
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
    fn scan_lock_key_uses_address_id() {
        let address_id = Uuid::from_u128(42);

        assert_eq!(
            scan_lock_key(address_id),
            "scan:address:lock:00000000-0000-0000-0000-00000000002a"
        );
    }

    #[test]
    fn queue_depth_command_uses_llen_and_queue_key() {
        assert_eq!(
            queue_depth_command("scan:address:queue"),
            ["LLEN", "scan:address:queue"]
        );
    }
}
