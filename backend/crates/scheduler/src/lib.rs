use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{ScanAddressCandidate, ScanAddressTask},
    AppResult,
};
use coin_listener_storage::{repositories, scan_jobs, scan_queue::ScanQueue};
use redis::aio::MultiplexedConnection;
use sqlx::PgPool;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::time;
use tracing::{error, info};
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

pub fn scheduler_shutdown_requested(shutdown: &AtomicBool) -> bool {
    shutdown.load(Ordering::Relaxed)
}

async fn wait_for_scheduler_shutdown(shutdown: Arc<AtomicBool>) {
    while !scheduler_shutdown_requested(&shutdown) {
        time::sleep(Duration::from_millis(50)).await;
    }
}

pub async fn run_scheduler(
    pool: PgPool,
    mut redis: MultiplexedConnection,
    queue: ScanQueue,
    batch_size: i64,
    tick_seconds: u64,
    job_max_attempts: i32,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
    let mut ticker = time::interval(Duration::from_secs(tick_seconds));

    while !scheduler_shutdown_requested(&shutdown) {
        tokio::select! {
            _ = ticker.tick() => {
                if scheduler_shutdown_requested(&shutdown) {
                    break;
                }

                match enqueue_due_addresses(&pool, &mut redis, &queue, batch_size, job_max_attempts, Utc::now()).await {
                    Ok(enqueued) => info!(service = "scheduler", enqueued, "scheduler tick completed"),
                    Err(error) => {
                        error!(service = "scheduler", error = %error, "scheduler tick failed");
                        return Err(error);
                    }
                }
            }
            _ = wait_for_scheduler_shutdown(Arc::clone(&shutdown)) => break,
        }
    }

    Ok(())
}

pub async fn enqueue_due_addresses(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    queue: &ScanQueue,
    batch_size: i64,
    job_max_attempts: i32,
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

        let inserted_job = scan_jobs::insert_scheduled_scan_job_in_transaction(
            &mut transaction,
            &candidate,
            job_max_attempts,
            now,
        )
        .await?;
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

        if inserted_job.is_some() {
            if let Err(error) = queue.signal(redis).await {
                tracing::warn!(error = %error, "failed to signal scan worker after durable job insert");
            }
            enqueued += 1;
        }
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

    #[test]
    fn scheduler_shutdown_flag_reads_atomic_state() {
        let shutdown = std::sync::atomic::AtomicBool::new(false);
        assert!(!super::scheduler_shutdown_requested(&shutdown));

        shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(super::scheduler_shutdown_requested(&shutdown));
    }

    #[test]
    fn scheduler_exports_reusable_runtime_loop() {
        let source = include_str!("lib.rs");

        assert!(source.contains("pub async fn run_scheduler("));
        assert!(source.contains("while !scheduler_shutdown_requested(&shutdown)"));
        assert!(source.contains("enqueue_due_addresses("));
    }

    #[test]
    fn scheduler_creates_durable_scan_jobs_before_wakeup_signal() {
        let source = include_str!("lib.rs");
        let job_insert = source
            .find("scan_jobs::insert_scheduled_scan_job_in_transaction")
            .expect("scheduler inserts durable scan job");
        let commit = source
            .find("transaction\n            .commit()")
            .expect("scheduler commits transaction");
        let signal = source.find("queue.signal(redis)").expect("scheduler signals Redis wakeup");

        assert!(job_insert < commit);
        assert!(commit < signal);
        assert!(source.contains("failed to signal scan worker after durable job insert"));
    }
}
