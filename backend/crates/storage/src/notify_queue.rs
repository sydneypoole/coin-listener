use coin_listener_core::{models::NotifyEventTask, AppError, AppResult};
use redis::{aio::MultiplexedConnection, Client};

#[derive(Debug, Clone)]
pub struct NotifyQueue {
    queue_key: String,
}

impl NotifyQueue {
    pub fn new(queue_key: String) -> Self {
        Self { queue_key }
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
        task: &NotifyEventTask,
    ) -> AppResult<()> {
        let payload = serialize_notify_task(task)?;
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
    ) -> AppResult<Option<NotifyEventTask>> {
        let result: Option<(String, String)> = redis::cmd("BRPOP")
            .arg(&self.queue_key)
            .arg(timeout_seconds)
            .query_async(connection)
            .await
            .map_err(|error| AppError::Redis(error.to_string()))?;

        result
            .map(|(_, payload)| deserialize_notify_task(&payload))
            .transpose()
    }
}

pub async fn connect_notify_queue(client: &Client) -> AppResult<MultiplexedConnection> {
    client
        .get_multiplexed_async_connection()
        .await
        .map_err(|error| AppError::Redis(error.to_string()))
}

pub fn queue_depth_command(queue_key: &str) -> [&str; 2] {
    ["LLEN", queue_key]
}

pub fn serialize_notify_task(task: &NotifyEventTask) -> AppResult<String> {
    serde_json::to_string(task).map_err(|error| AppError::Validation(error.to_string()))
}

pub fn deserialize_notify_task(payload: &str) -> AppResult<NotifyEventTask> {
    serde_json::from_str(payload).map_err(|error| AppError::Validation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{deserialize_notify_task, queue_depth_command, serialize_notify_task};
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::NotifyEventTask;
    use uuid::Uuid;

    #[test]
    fn notify_task_payload_round_trips() {
        let task = NotifyEventTask {
            task_id: Uuid::from_u128(21),
            event_id: Uuid::from_u128(22),
            tenant_id: Uuid::from_u128(23),
            attempt: 1,
            enqueued_at: Utc.with_ymd_and_hms(2026, 5, 17, 16, 0, 0).unwrap(),
        };

        let payload = serialize_notify_task(&task).expect("serialize task");
        let decoded = deserialize_notify_task(&payload).expect("deserialize task");

        assert_eq!(decoded, task);
    }

    #[test]
    fn malformed_notify_task_payload_returns_error() {
        let result = deserialize_notify_task("not-json");

        assert!(result.is_err());
    }

    #[test]
    fn queue_depth_command_uses_llen_and_queue_key() {
        assert_eq!(
            queue_depth_command("notify:event:queue"),
            ["LLEN", "notify:event:queue"]
        );
    }
}
