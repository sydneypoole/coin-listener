use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{ScanAddressCandidate, ScanAddressTask},
    AppResult,
};
use coin_listener_storage::{repositories, scan_queue::ScanQueue};
use redis::aio::MultiplexedConnection;
use sqlx::PgPool;
use uuid::Uuid;

pub fn build_scan_task(candidate: &ScanAddressCandidate, now: DateTime<Utc>) -> ScanAddressTask {
    ScanAddressTask {
        task_id: Uuid::new_v4(),
        address_id: candidate.id,
        tenant_id: candidate.tenant_id,
        chain_id: candidate.chain_id,
        attempt: 1,
        enqueued_at: now,
    }
}

pub async fn enqueue_due_addresses(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    queue: &ScanQueue,
    batch_size: i64,
    now: DateTime<Utc>,
) -> AppResult<usize> {
    let mut enqueued = 0usize;

    for _ in 0..batch_size {
        let mut transaction = pool
            .begin()
            .await
            .map_err(|error| coin_listener_core::AppError::Database(error.to_string()))?;
        let Some(candidate) =
            repositories::claim_one_due_scan_address_for_update(&mut transaction, now).await?
        else {
            transaction
                .rollback()
                .await
                .map_err(|error| coin_listener_core::AppError::Database(error.to_string()))?;
            break;
        };

        let task = build_scan_task(&candidate, now);
        queue.enqueue(redis, &task).await?;
        repositories::mark_claimed_scan_enqueued(
            &mut transaction,
            candidate.id,
            repositories::next_scan_at_from(now, candidate.scan_interval_seconds),
        )
        .await?;
        transaction
            .commit()
            .await
            .map_err(|error| coin_listener_core::AppError::Database(error.to_string()))?;
        enqueued += 1;
    }

    Ok(enqueued)
}

#[cfg(test)]
mod tests {
    use super::build_scan_task;
    use chrono::{TimeZone, Utc};
    use coin_listener_core::models::ScanAddressCandidate;
    use uuid::Uuid;

    #[test]
    fn build_scan_task_uses_candidate_ids_and_first_attempt() {
        let candidate = ScanAddressCandidate {
            id: Uuid::from_u128(2),
            tenant_id: Uuid::from_u128(3),
            chain_id: Uuid::from_u128(4),
            scan_interval_seconds: 300,
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 17, 14, 0, 0).unwrap();

        let task = build_scan_task(&candidate, now);

        assert_eq!(task.address_id, candidate.id);
        assert_eq!(task.tenant_id, candidate.tenant_id);
        assert_eq!(task.chain_id, candidate.chain_id);
        assert_eq!(task.attempt, 1);
        assert_eq!(task.enqueued_at, now);
    }
}
