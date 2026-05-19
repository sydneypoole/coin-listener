use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use std::time::Duration;

use chrono::{DateTime, Utc};
use coin_listener_chain_providers::{
    btc::{self, BtcClient},
    evm::{
        self, address_to_topic, evm_balance_change_event, parse_hex_u256_to_decimal_string,
        transfer_event_draft, wei_to_decimal_string, EvmBlockTag, EvmLogFilter, EvmRpcClient,
        TRANSFER_TOPIC0,
    },
    tron::{self, TronClient},
};
use coin_listener_core::{
    models::{
        AddressEvent, Asset, BalanceSnapshot, CreateBalanceSnapshotRequest, Provider,
        ScanAddressContext, ScanAddressTask, ScanCursor,
    },
    AppError, AppResult,
};
use coin_listener_storage::{repositories, scan_queue::ScanQueue};
use redis::aio::MultiplexedConnection;
use sqlx::PgPool;
use tracing::{info, warn};

pub const EVM_ERC20_TRANSFER_CURSOR: &str = "evm_erc20_transfer";
pub const EVM_TRANSFER_INITIAL_WINDOW_BLOCKS: i64 = 1_000;
pub const TRON_TRX_TRANSFER_CURSOR: &str = "tron_trx_transfer";
pub const TRON_TRC20_TRANSFER_CURSOR: &str = "tron_trc20_transfer";
pub const BTC_TRANSACTION_CURSOR: &str = "btc_transaction";
pub const TRON_INITIAL_WATERMARK_WINDOW: i64 = 86_400_000;
pub const BTC_INITIAL_BLOCK_WINDOW: i64 = 3;
pub const MAX_PROVIDER_PAGES_PER_SCAN: usize = 10;
pub const BTC_CURSOR_OVERLAP_BLOCKS: i64 = 1;
pub const PROVIDER_PAGE_LOG_INDEX_STRIDE: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanPlan {
    EvmNativeBalance,
    Tron,
    Btc,
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanTaskOutcome {
    Locked,
    Scanned,
    UnsupportedChain(String),
}

pub fn scan_plan_for_chain(chain_type: &str) -> ScanPlan {
    match chain_type {
        "evm" => ScanPlan::EvmNativeBalance,
        "tron" => ScanPlan::Tron,
        "utxo" => ScanPlan::Btc,
        other => ScanPlan::Unsupported(other.to_string()),
    }
}

pub fn worker_shutdown_requested(shutdown: &AtomicBool) -> bool {
    shutdown.load(Ordering::Relaxed)
}

pub fn should_emit_balance_change(
    previous: Option<&BalanceSnapshot>,
    current: &BalanceSnapshot,
) -> bool {
    previous.is_some_and(|previous| previous.balance_raw != current.balance_raw)
}

pub fn evm_transfer_scan_range(
    cursor: Option<&ScanCursor>,
    latest_block: i64,
    default_confirmations: i32,
) -> AppResult<Option<(i64, i64)>> {
    if default_confirmations < 0 {
        return Err(AppError::Validation(
            "default_confirmations cannot be negative".to_string(),
        ));
    }
    let confirmed_to = latest_block - i64::from(default_confirmations);
    if confirmed_to < 0 {
        return Ok(None);
    }
    let from_block = cursor
        .map(|cursor| cursor.last_scanned_block + 1)
        .unwrap_or_else(|| (confirmed_to - EVM_TRANSFER_INITIAL_WINDOW_BLOCKS + 1).max(0));
    if confirmed_to < from_block {
        return Ok(None);
    }
    Ok(Some((from_block, confirmed_to)))
}

pub fn confirmed_cursor_range(
    cursor: Option<&ScanCursor>,
    latest_value: i64,
    confirmations: i64,
    initial_window: i64,
    label: &str,
) -> AppResult<Option<(i64, i64)>> {
    if confirmations < 0 {
        return Err(AppError::Validation(format!(
            "{label} confirmations cannot be negative"
        )));
    }
    if initial_window <= 0 {
        return Err(AppError::Validation(
            "initial_window must be positive".to_string(),
        ));
    }

    let confirmed_to = latest_value - confirmations;
    if confirmed_to < 0 {
        return Ok(None);
    }

    let from = cursor
        .map(|cursor| cursor.last_scanned_block + 1)
        .unwrap_or_else(|| (confirmed_to - initial_window + 1).max(0));
    if confirmed_to < from {
        return Ok(None);
    }
    Ok(Some((from, confirmed_to)))
}

pub fn tron_cursor_value(transfers: &[tron::DecodedTronTransfer]) -> Option<i64> {
    transfers.iter().map(|transfer| transfer.cursor_value).max()
}

pub fn tron_transfer_should_scan(asset: &Asset, transfer: &tron::DecodedTronTransfer) -> bool {
    match (&asset.contract_address, &transfer.token_contract) {
        (Some(asset_contract), Some(transfer_contract)) => asset_contract == transfer_contract,
        (None, None) => asset.asset_type == "native",
        _ => false,
    }
}

fn collect_matching_tron_transfer(
    asset: &Asset,
    transfer: tron::DecodedTronTransfer,
    cursor_value: &mut Option<i64>,
    transfers: &mut Vec<(Asset, tron::DecodedTronTransfer)>,
) {
    if !tron_transfer_should_scan(asset, &transfer) {
        return;
    }

    *cursor_value = Some(
        cursor_value
            .map(|current| current.max(transfer.cursor_value))
            .unwrap_or(transfer.cursor_value),
    );
    transfers.push((asset.clone(), transfer));
}

pub fn btc_cursor_value(transfers: &[btc::DecodedBtcTransfer]) -> Option<i64> {
    transfers.iter().map(|transfer| transfer.block_number).max()
}

pub fn btc_scan_from_block(cursor: Option<&ScanCursor>) -> i64 {
    cursor
        .map(|cursor| {
            cursor
                .last_scanned_block
                .saturating_sub(BTC_CURSOR_OVERLAP_BLOCKS.saturating_sub(1))
                .max(0)
        })
        .unwrap_or(0)
}

pub fn paged_log_index(page_index: usize, item_index: usize) -> AppResult<i32> {
    let value = page_index
        .checked_mul(PROVIDER_PAGE_LOG_INDEX_STRIDE)
        .and_then(|base| base.checked_add(item_index))
        .ok_or_else(|| AppError::Validation("provider page item index overflow".to_string()))?;
    i32::try_from(value)
        .map_err(|_| AppError::Validation("provider page item index overflow".to_string()))
}

pub fn ensure_provider_page_limit(
    label: &str,
    pages_processed: usize,
    has_next_page: bool,
) -> AppResult<()> {
    if has_next_page && pages_processed >= MAX_PROVIDER_PAGES_PER_SCAN {
        return Err(AppError::Config(format!(
            "{label} pagination exceeded max page limit {MAX_PROVIDER_PAGES_PER_SCAN}"
        )));
    }
    Ok(())
}

pub fn is_provider_availability_error(error: &AppError) -> bool {
    let AppError::Config(message) = error else {
        return false;
    };

    message.contains("request failed")
        || message.contains("response body failed")
        || message.contains("returned http")
        || message.starts_with("no active rpc provider capacity for chain ")
}

pub fn provider_capacity_error(chain_id: uuid::Uuid) -> AppError {
    AppError::Config(format!(
        "no active rpc provider capacity for chain {chain_id}"
    ))
}

pub fn provider_timeout_duration(provider: &Provider) -> AppResult<Duration> {
    let timeout_ms = u64::try_from(provider.timeout_ms)
        .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
    if timeout_ms == 0 {
        return Err(AppError::Validation(
            "timeout_ms must be positive".to_string(),
        ));
    }
    Ok(Duration::from_millis(timeout_ms))
}

fn should_process_btc_transaction_page(
    pages_processed: usize,
    transaction_count: usize,
) -> AppResult<bool> {
    if transaction_count == 0 {
        return Ok(false);
    }
    if pages_processed >= MAX_PROVIDER_PAGES_PER_SCAN {
        ensure_provider_page_limit("BTC address transactions", pages_processed, true)?;
    }
    Ok(true)
}

pub async fn scan_evm_native_balance(
    pool: &PgPool,
    task: &ScanAddressTask,
    _now: DateTime<Utc>,
) -> AppResult<Option<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let provider = repositories::active_rpc_provider_for_chain(pool, context.chain_id).await?;
    let timeout = provider_timeout_duration(&provider)?;

    let rpc = EvmRpcClient::new(provider.base_url.clone(), timeout);
    let block_number = rpc.eth_block_number().await?;
    scan_evm_native_balance_with_context(pool, &rpc, &context, &asset, &provider, block_number)
        .await
}

async fn scan_evm_native_balance_with_context(
    pool: &PgPool,
    rpc: &EvmRpcClient,
    context: &ScanAddressContext,
    asset: &Asset,
    provider: &Provider,
    block_number: i64,
) -> AppResult<Option<AddressEvent>> {
    let balance_hex = rpc
        .eth_get_balance(&context.address, EvmBlockTag::Latest)
        .await?;
    let balance_raw = parse_hex_u256_to_decimal_string(&balance_hex)?;
    let balance_decimal = wei_to_decimal_string(&balance_raw, asset.decimals)?;
    let current = repositories::insert_balance_snapshot(
        pool,
        CreateBalanceSnapshotRequest {
            tenant_id: context.tenant_id,
            chain_id: context.chain_id,
            address_id: context.id,
            asset_id: asset.id,
            balance_raw,
            balance_decimal,
            block_number: Some(block_number),
            block_hash: None,
            source_provider_id: Some(provider.id),
        },
    )
    .await?;
    let previous =
        repositories::latest_balance_snapshot(pool, context.id, asset.id, Some(current.id)).await?;
    if !should_emit_balance_change(previous.as_ref(), &current) {
        return Ok(None);
    }

    let previous = previous.expect("previous snapshot checked before event creation");
    let draft = evm_balance_change_event(context, asset, &previous, &current, provider)?;
    repositories::insert_event_and_outbox_if_not_exists(pool, draft).await
}

pub async fn scan_evm_erc20_transfers(
    pool: &PgPool,
    rpc: &EvmRpcClient,
    context: &ScanAddressContext,
    latest_block: i64,
    default_confirmations: i32,
) -> AppResult<Vec<AddressEvent>> {
    let cursor = repositories::scan_cursor(pool, context.id, EVM_ERC20_TRANSFER_CURSOR).await?;
    let Some((from_block, to_block)) =
        evm_transfer_scan_range(cursor.as_ref(), latest_block, default_confirmations)?
    else {
        return Ok(Vec::new());
    };

    let assets = repositories::active_erc20_assets_for_chain(pool, context.chain_id).await?;
    if assets.is_empty() {
        return Ok(Vec::new());
    }

    let watched_topic = address_to_topic(&context.address)?;
    let mut events = Vec::new();

    for asset in assets {
        let Some(contract_address) = asset.contract_address.clone() else {
            continue;
        };
        let incoming = EvmLogFilter {
            address: contract_address.clone(),
            from_block,
            to_block,
            topics: vec![
                Some(TRANSFER_TOPIC0.to_string()),
                None,
                Some(watched_topic.clone()),
            ],
        };
        let outgoing = EvmLogFilter {
            address: contract_address,
            from_block,
            to_block,
            topics: vec![
                Some(TRANSFER_TOPIC0.to_string()),
                Some(watched_topic.clone()),
                None,
            ],
        };

        for filter in [incoming, outgoing] {
            let logs = rpc.eth_get_logs(filter).await?;
            for log in logs {
                let transfer = evm::decode_erc20_transfer_log(&log, asset.decimals)?;
                let draft = transfer_event_draft(context, &asset, transfer);
                if let Some(event) =
                    repositories::insert_event_and_outbox_if_not_exists(pool, draft).await?
                {
                    events.push(event);
                }
            }
        }
    }

    repositories::upsert_scan_cursor(
        pool,
        context.tenant_id,
        context.chain_id,
        context.id,
        EVM_ERC20_TRANSFER_CURSOR,
        to_block,
    )
    .await?;

    Ok(events)
}

pub async fn scan_evm_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    _now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let _ = redis;
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let provider = repositories::active_rpc_provider_for_chain(pool, context.chain_id).await?;
    let chain = repositories::chain_by_id(pool, context.chain_id).await?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let timeout = provider_timeout_duration(&provider)?;
    let rpc = EvmRpcClient::new(provider.base_url.clone(), timeout);
    let latest_block = rpc.eth_block_number().await?;

    let mut events = Vec::new();
    if let Some(event) = scan_evm_native_balance_with_context(
        pool,
        &rpc,
        &context,
        &native_asset,
        &provider,
        latest_block,
    )
    .await?
    {
        events.push(event);
    }
    events.extend(
        scan_evm_erc20_transfers(
            pool,
            &rpc,
            &context,
            latest_block,
            chain.default_confirmations,
        )
        .await?,
    );
    Ok(events)
}

pub async fn scan_tron_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    _now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let _ = redis;
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let provider = repositories::active_rpc_provider_for_chain(pool, context.chain_id).await?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let timeout = provider_timeout_duration(&provider)?;

    let client = TronClient::new(provider.base_url.clone(), timeout);

    let trx_cursor = repositories::scan_cursor(pool, context.id, TRON_TRX_TRANSFER_CURSOR).await?;
    let trx_from = trx_cursor
        .as_ref()
        .map(|cursor| cursor.last_scanned_block + 1)
        .unwrap_or(0);
    let mut trx_transfers = Vec::new();
    let mut trx_fingerprint: Option<String> = None;
    let mut trx_pages_processed = 0usize;

    loop {
        let page = client
            .account_transactions_page(&context.address, trx_from, trx_fingerprint.as_deref())
            .await?;
        let page_index = trx_pages_processed;
        trx_pages_processed += 1;
        let tron::TronPage {
            data,
            next_fingerprint,
        } = page;
        let has_next_page = next_fingerprint.is_some();
        ensure_provider_page_limit(
            "TRON account transactions",
            trx_pages_processed,
            has_next_page,
        )?;

        for (index, payload) in data.iter().enumerate() {
            match tron::try_decode_trx_transfer_at_index(
                payload,
                native_asset.decimals,
                paged_log_index(page_index, index)?,
            )? {
                tron::TrxTransferDecode::Transfer(transfer) => trx_transfers.push(transfer),
                tron::TrxTransferDecode::Skip => continue,
            }
        }

        let Some(next) = next_fingerprint else {
            break;
        };
        trx_fingerprint = Some(next);
    }

    let trx_cursor_value = tron_cursor_value(&trx_transfers);

    let trc20_assets =
        repositories::active_assets_for_chain_by_type(pool, context.chain_id, "trc20").await?;
    let trc20_cursor =
        repositories::scan_cursor(pool, context.id, TRON_TRC20_TRANSFER_CURSOR).await?;
    let trc20_from = trc20_cursor
        .as_ref()
        .map(|cursor| cursor.last_scanned_block + 1)
        .unwrap_or(0);
    let mut trc20_cursor_value: Option<i64> = None;
    let mut trc20_transfers = Vec::new();
    for asset in trc20_assets {
        let Some(contract_address) = asset.contract_address.clone() else {
            continue;
        };
        let mut trc20_fingerprint: Option<String> = None;
        let mut trc20_pages_processed = 0usize;

        loop {
            let page = client
                .account_trc20_transfers_page(
                    &context.address,
                    &contract_address,
                    trc20_from,
                    trc20_fingerprint.as_deref(),
                )
                .await?;
            let page_index = trc20_pages_processed;
            trc20_pages_processed += 1;
            let tron::TronPage {
                data,
                next_fingerprint,
            } = page;
            let has_next_page = next_fingerprint.is_some();
            ensure_provider_page_limit(
                "TRON TRC20 transfers",
                trc20_pages_processed,
                has_next_page,
            )?;

            for (index, payload) in data.into_iter().enumerate() {
                let transfer = tron::decode_trc20_transfer_at_index(
                    &payload,
                    &contract_address,
                    asset.decimals,
                    paged_log_index(page_index, index)?,
                )?;
                collect_matching_tron_transfer(
                    &asset,
                    transfer,
                    &mut trc20_cursor_value,
                    &mut trc20_transfers,
                );
            }

            let Some(next) = next_fingerprint else {
                break;
            };
            trc20_fingerprint = Some(next);
        }
    }

    let mut events = Vec::new();
    for transfer in trx_transfers {
        if !tron_transfer_should_scan(&native_asset, &transfer) {
            continue;
        }
        let draft = tron::tron_transfer_event_draft(&context, &native_asset, transfer);
        if let Some(event) =
            repositories::insert_event_and_outbox_if_not_exists(pool, draft).await?
        {
            events.push(event);
        }
    }
    for (asset, transfer) in trc20_transfers {
        let draft = tron::tron_transfer_event_draft(&context, &asset, transfer);
        if let Some(event) =
            repositories::insert_event_and_outbox_if_not_exists(pool, draft).await?
        {
            events.push(event);
        }
    }

    if let Some(cursor_value) = trx_cursor_value {
        repositories::upsert_scan_cursor(
            pool,
            context.tenant_id,
            context.chain_id,
            context.id,
            TRON_TRX_TRANSFER_CURSOR,
            cursor_value,
        )
        .await?;
    }
    if let Some(cursor_value) = trc20_cursor_value {
        repositories::upsert_scan_cursor(
            pool,
            context.tenant_id,
            context.chain_id,
            context.id,
            TRON_TRC20_TRANSFER_CURSOR,
            cursor_value,
        )
        .await?;
    }

    Ok(events)
}

pub async fn scan_btc_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    _now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let _ = redis;
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let provider = repositories::active_rpc_provider_for_chain(pool, context.chain_id).await?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let timeout = provider_timeout_duration(&provider)?;

    let client = BtcClient::new(provider.base_url.clone(), timeout);
    let balance = client.address_balance(&context.address).await?;
    repositories::insert_balance_snapshot(
        pool,
        CreateBalanceSnapshotRequest {
            tenant_id: context.tenant_id,
            chain_id: context.chain_id,
            address_id: context.id,
            asset_id: native_asset.id,
            balance_raw: balance.balance_raw,
            balance_decimal: balance.balance_decimal,
            block_number: None,
            block_hash: None,
            source_provider_id: Some(provider.id),
        },
    )
    .await?;

    let cursor = repositories::scan_cursor(pool, context.id, BTC_TRANSACTION_CURSOR).await?;
    let from_block = btc_scan_from_block(cursor.as_ref());
    let mut events = Vec::new();
    let mut transfers = Vec::new();
    let mut next_last_seen_txid: Option<String> = None;
    let mut pages_processed = 0usize;

    loop {
        let page = client
            .address_transactions_page(&context.address, next_last_seen_txid.as_deref())
            .await?;
        if !should_process_btc_transaction_page(pages_processed, page.transactions.len())? {
            break;
        }
        pages_processed += 1;

        for tx in page.transactions {
            let Some(transfer) = btc::classify_btc_transaction(&tx, &context.address)? else {
                continue;
            };
            if transfer.block_number < from_block {
                continue;
            }
            transfers.push(transfer);
        }

        let Some(next) = page.next_last_seen_txid else {
            break;
        };
        next_last_seen_txid = Some(next);
    }

    let cursor_value = btc_cursor_value(&transfers);
    for transfer in transfers {
        let draft = btc::btc_transfer_event_draft(&context, &native_asset, transfer);
        if let Some(event) =
            repositories::insert_event_and_outbox_if_not_exists(pool, draft).await?
        {
            events.push(event);
        }
    }

    if let Some(cursor_value) = cursor_value {
        repositories::upsert_scan_cursor(
            pool,
            context.tenant_id,
            context.chain_id,
            context.id,
            BTC_TRANSACTION_CURSOR,
            cursor_value,
        )
        .await?;
    }

    Ok(events)
}

pub async fn process_scan_task(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    scan_queue: &ScanQueue,
    task: ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
    let acquired = scan_queue
        .acquire_lock(redis, task.address_id, task.task_id)
        .await?;
    if !acquired {
        return Ok(ScanTaskOutcome::Locked);
    }

    let outcome = process_locked_scan_task(pool, redis, &task, now).await;
    if let Err(error) = scan_queue
        .release_lock(redis, task.address_id, task.task_id)
        .await
    {
        warn!(
            task_id = %task.task_id,
            address_id = %task.address_id,
            error = %error,
            "failed to release scan lock"
        );
    }

    outcome
}

async fn process_locked_scan_task(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
    let next_scan_at = repositories::next_scan_at_from(now, context.scan_interval_seconds);

    match scan_plan_for_chain(&context.chain_type) {
        ScanPlan::EvmNativeBalance => {
            let _events = scan_evm_address(pool, redis, task, now).await?;
            repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
            Ok(ScanTaskOutcome::Scanned)
        }
        ScanPlan::Tron => {
            let _events = scan_tron_address(pool, redis, task, now).await?;
            repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
            Ok(ScanTaskOutcome::Scanned)
        }
        ScanPlan::Btc => {
            let _events = scan_btc_address(pool, redis, task, now).await?;
            repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
            Ok(ScanTaskOutcome::Scanned)
        }
        ScanPlan::Unsupported(chain_type) => {
            warn!(
                task_id = %task.task_id,
                address_id = %task.address_id,
                chain_type,
                "chain type is not supported by worker scan"
            );
            repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
            Ok(ScanTaskOutcome::UnsupportedChain(chain_type))
        }
    }
}

pub async fn run_worker(
    pool: PgPool,
    mut redis: MultiplexedConnection,
    scan_queue: ScanQueue,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
    while !worker_shutdown_requested(&shutdown) {
        match scan_queue.dequeue(&mut redis, 5).await {
            Ok(Some(task)) => {
                let task_id = task.task_id;
                let address_id = task.address_id;
                match process_scan_task(&pool, &mut redis, &scan_queue, task, Utc::now()).await {
                    Ok(outcome) => info!(
                        task_id = %task_id,
                        address_id = %address_id,
                        ?outcome,
                        "scan task processed"
                    ),
                    Err(error) => warn!(
                        task_id = %task_id,
                        address_id = %address_id,
                        error = %error,
                        "scan task failed"
                    ),
                }
            }
            Ok(None) => {}
            Err(error) => warn!(error = %error, "discarded invalid or failed scan queue message"),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    mod scan_plan_for_chain {
        use crate::{scan_plan_for_chain, ScanPlan};

        #[test]
        fn evm_chain_uses_native_balance_scan() {
            assert_eq!(scan_plan_for_chain("evm"), ScanPlan::EvmNativeBalance);
        }

        #[test]
        fn tron_chain_uses_tron_scan() {
            assert_eq!(scan_plan_for_chain("tron"), ScanPlan::Tron);
        }

        #[test]
        fn utxo_chain_uses_btc_scan() {
            assert_eq!(scan_plan_for_chain("utxo"), ScanPlan::Btc);
        }

        #[test]
        fn unknown_chain_is_unsupported() {
            assert_eq!(
                scan_plan_for_chain("solana"),
                ScanPlan::Unsupported("solana".to_string())
            );
        }
    }

    mod provider_failover_helpers {
        use coin_listener_core::{models::Provider, AppError};
        use uuid::Uuid;

        use crate::{
            is_provider_availability_error, provider_capacity_error, provider_timeout_duration,
        };

        #[test]
        fn config_errors_are_provider_availability_errors() {
            assert!(is_provider_availability_error(&AppError::Config(
                "provider request failed: timeout".to_string()
            )));
        }

        #[test]
        fn validation_database_redis_and_worker_config_errors_do_not_fallback() {
            assert!(!is_provider_availability_error(&AppError::Validation(
                "bad decoded data".to_string()
            )));
            assert!(!is_provider_availability_error(&AppError::Database(
                "db unavailable".to_string()
            )));
            assert!(!is_provider_availability_error(&AppError::Redis(
                "redis unavailable".to_string()
            )));
            assert!(!is_provider_availability_error(&AppError::Config(
                "TRON account transactions pagination exceeded max page limit 10".to_string()
            )));
        }

        #[test]
        fn provider_capacity_error_names_chain() {
            let error = provider_capacity_error(Uuid::from_u128(7));

            assert!(
                matches!(error, AppError::Config(message) if message.contains("no active rpc provider capacity for chain 00000000-0000-0000-0000-000000000007"))
            );
        }

        #[test]
        fn provider_capacity_errors_are_availability_errors() {
            let error = provider_capacity_error(Uuid::from_u128(7));

            assert!(is_provider_availability_error(&error));
        }

        #[test]
        fn provider_timeout_duration_rejects_zero_or_negative_values() {
            let provider = provider_with_timeout(0);
            let error = provider_timeout_duration(&provider).unwrap_err();
            assert!(
                matches!(error, AppError::Validation(message) if message == "timeout_ms must be positive")
            );

            let provider = provider_with_timeout(-1);
            let error = provider_timeout_duration(&provider).unwrap_err();
            assert!(
                matches!(error, AppError::Validation(message) if message == "timeout_ms must be positive")
            );
        }

        #[test]
        fn provider_timeout_duration_accepts_positive_values() {
            let provider = provider_with_timeout(1500);

            assert_eq!(
                provider_timeout_duration(&provider).unwrap().as_millis(),
                1500
            );
        }

        fn provider_with_timeout(timeout_ms: i32) -> Provider {
            Provider {
                id: Uuid::from_u128(1),
                chain_id: Uuid::from_u128(2),
                provider_type: "rpc".to_string(),
                name: "provider".to_string(),
                base_url: "https://example.invalid".to_string(),
                api_key_ref: None,
                priority: 1,
                qps_limit: 10,
                timeout_ms,
                status: "active".to_string(),
            }
        }
    }

    mod worker_shutdown_requested {
        use std::sync::atomic::AtomicBool;

        use crate::worker_shutdown_requested;

        #[test]
        fn set_shutdown_flag_stops_before_next_dequeue() {
            let shutdown = AtomicBool::new(true);

            assert!(worker_shutdown_requested(&shutdown));
        }

        #[test]
        fn unset_shutdown_flag_continues_worker_loop() {
            let shutdown = AtomicBool::new(false);

            assert!(!worker_shutdown_requested(&shutdown));
        }
    }

    mod balance_change_gating {
        use chrono::{TimeZone, Utc};
        use coin_listener_core::models::BalanceSnapshot;
        use uuid::Uuid;

        use crate::should_emit_balance_change;

        fn snapshot(balance_raw: &str) -> BalanceSnapshot {
            BalanceSnapshot {
                id: Uuid::new_v4(),
                tenant_id: Uuid::from_u128(1),
                chain_id: Uuid::from_u128(2),
                address_id: Uuid::from_u128(3),
                asset_id: Uuid::from_u128(4),
                balance_raw: balance_raw.to_string(),
                balance_decimal: balance_raw.to_string(),
                block_number: Some(100),
                block_hash: None,
                observed_at: Utc.with_ymd_and_hms(2026, 5, 17, 18, 0, 0).unwrap(),
                source_provider_id: Some(Uuid::from_u128(5)),
            }
        }

        #[test]
        fn first_snapshot_does_not_emit_balance_change() {
            let current = snapshot("100");

            assert!(!should_emit_balance_change(None, &current));
        }

        #[test]
        fn unchanged_raw_balance_does_not_emit_balance_change() {
            let previous = snapshot("100");
            let current = snapshot("100");

            assert!(!should_emit_balance_change(Some(&previous), &current));
        }

        #[test]
        fn changed_raw_balance_emits_balance_change() {
            let previous = snapshot("100");
            let current = snapshot("101");

            assert!(should_emit_balance_change(Some(&previous), &current));
        }
    }

    mod evm_transfer_ranges {
        use chrono::{TimeZone, Utc};
        use coin_listener_core::models::ScanCursor;
        use uuid::Uuid;

        use crate::{evm_transfer_scan_range, EVM_ERC20_TRANSFER_CURSOR};

        fn cursor(last_scanned_block: i64) -> ScanCursor {
            ScanCursor {
                id: Uuid::from_u128(1),
                tenant_id: Uuid::from_u128(2),
                chain_id: Uuid::from_u128(3),
                address_id: Uuid::from_u128(4),
                cursor_type: EVM_ERC20_TRANSFER_CURSOR.to_string(),
                last_scanned_block,
                updated_at: Utc.with_ymd_and_hms(2026, 5, 17, 23, 0, 0).unwrap(),
            }
        }

        #[test]
        fn initial_range_uses_latest_confirmed_window() {
            let range = evm_transfer_scan_range(None, 20_000, 12).unwrap();

            assert_eq!(range, Some((18_989, 19_988)));
        }

        #[test]
        fn cursor_range_starts_after_last_scanned_block() {
            let range = evm_transfer_scan_range(Some(&cursor(19_900)), 20_000, 12).unwrap();

            assert_eq!(range, Some((19_901, 19_988)));
        }

        #[test]
        fn cursor_ahead_of_confirmed_block_is_noop() {
            let range = evm_transfer_scan_range(Some(&cursor(19_988)), 20_000, 12).unwrap();

            assert_eq!(range, None);
        }

        #[test]
        fn confirmations_cannot_be_negative() {
            let error = evm_transfer_scan_range(None, 20_000, -1).unwrap_err();

            assert!(error.to_string().contains("default_confirmations"));
        }
    }

    mod cursor_ranges {
        use chrono::{TimeZone, Utc};
        use coin_listener_core::models::ScanCursor;
        use uuid::Uuid;

        use crate::{
            confirmed_cursor_range, BTC_INITIAL_BLOCK_WINDOW, BTC_TRANSACTION_CURSOR,
            TRON_INITIAL_WATERMARK_WINDOW,
        };

        fn cursor(last_scanned_block: i64) -> ScanCursor {
            ScanCursor {
                id: Uuid::from_u128(1),
                tenant_id: Uuid::from_u128(2),
                chain_id: Uuid::from_u128(3),
                address_id: Uuid::from_u128(4),
                cursor_type: BTC_TRANSACTION_CURSOR.to_string(),
                last_scanned_block,
                updated_at: Utc.with_ymd_and_hms(2026, 5, 17, 23, 0, 0).unwrap(),
            }
        }

        #[test]
        fn initial_range_uses_confirmed_window() {
            let range =
                confirmed_cursor_range(None, 100_000, 3, BTC_INITIAL_BLOCK_WINDOW, "btc").unwrap();

            assert_eq!(range, Some((99_995, 99_997)));
        }

        #[test]
        fn initial_range_clamps_start_to_zero() {
            let range = confirmed_cursor_range(None, 100, 0, TRON_INITIAL_WATERMARK_WINDOW, "tron")
                .unwrap();

            assert_eq!(range, Some((0, 100)));
        }

        #[test]
        fn cursor_range_starts_after_last_scanned_block() {
            let range =
                confirmed_cursor_range(Some(&cursor(99_990)), 100_000, 3, 3, "btc").unwrap();

            assert_eq!(range, Some((99_991, 99_997)));
        }

        #[test]
        fn current_cursor_returns_none() {
            let range =
                confirmed_cursor_range(Some(&cursor(99_997)), 100_000, 3, 3, "btc").unwrap();

            assert_eq!(range, None);
        }

        #[test]
        fn negative_confirmations_error_includes_label() {
            let error = confirmed_cursor_range(None, 100_000, -1, 3, "tron").unwrap_err();

            assert!(error.to_string().contains("tron confirmations"));
        }

        #[test]
        fn non_positive_initial_window_is_rejected() {
            let error = confirmed_cursor_range(None, 100_000, 0, 0, "btc").unwrap_err();

            assert!(error.to_string().contains("initial_window"));
        }
    }

    mod tron_worker_helpers {
        use coin_listener_chain_providers::tron::DecodedTronTransfer;
        use coin_listener_core::models::Asset;
        use uuid::Uuid;

        use crate::{
            collect_matching_tron_transfer, ensure_provider_page_limit, paged_log_index,
            tron_cursor_value, tron_transfer_should_scan, MAX_PROVIDER_PAGES_PER_SCAN,
            TRON_TRC20_TRANSFER_CURSOR, TRON_TRX_TRANSFER_CURSOR,
        };

        const TOKEN_CONTRACT: &str = "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE";
        const OTHER_TOKEN_CONTRACT: &str = "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh";

        fn asset(contract_address: Option<&str>) -> Asset {
            Asset {
                id: Uuid::from_u128(1),
                chain_id: Uuid::from_u128(2),
                asset_type: contract_address.map_or("native", |_| "trc20").to_string(),
                symbol: "TEST".to_string(),
                name: "Test Asset".to_string(),
                contract_address: contract_address.map(ToString::to_string),
                decimals: 6,
                is_builtin: true,
                status: "active".to_string(),
            }
        }

        fn transfer(token_contract: Option<&str>, cursor_value: i64) -> DecodedTronTransfer {
            DecodedTronTransfer {
                tx_hash: format!("tx-{cursor_value}"),
                cursor_value,
                block_number: Some(cursor_value),
                log_index: Some(0),
                from_address: "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh".to_string(),
                to_address: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".to_string(),
                amount_raw: "1000000".to_string(),
                amount_decimal: "1".to_string(),
                token_contract: token_contract.map(ToString::to_string),
            }
        }

        #[test]
        fn cursor_value_returns_highest_transfer_cursor() {
            let transfers = vec![transfer(None, 12), transfer(None, 30), transfer(None, 20)];

            assert_eq!(tron_cursor_value(&transfers), Some(30));
        }

        #[test]
        fn cursor_value_returns_none_for_empty_transfers() {
            assert_eq!(tron_cursor_value(&[]), None);
        }

        #[test]
        fn native_asset_scans_native_transfer() {
            assert!(tron_transfer_should_scan(&asset(None), &transfer(None, 1)));
        }

        #[test]
        fn contractless_non_native_asset_skips_native_transfer() {
            let mut asset = asset(None);
            asset.asset_type = "trc20".to_string();

            assert!(!tron_transfer_should_scan(&asset, &transfer(None, 1)));
        }

        #[test]
        fn trc20_asset_scans_matching_token_contract() {
            assert!(tron_transfer_should_scan(
                &asset(Some(TOKEN_CONTRACT)),
                &transfer(Some(TOKEN_CONTRACT), 1)
            ));
        }

        #[test]
        fn trc20_asset_skips_different_token_contract() {
            assert!(!tron_transfer_should_scan(
                &asset(Some(TOKEN_CONTRACT)),
                &transfer(Some(OTHER_TOKEN_CONTRACT), 1)
            ));
        }

        #[test]
        fn native_asset_skips_token_transfer() {
            assert!(!tron_transfer_should_scan(
                &asset(None),
                &transfer(Some(TOKEN_CONTRACT), 1)
            ));
        }

        #[test]
        fn trc20_cursor_updates_only_for_matching_transfers() {
            let asset = asset(Some(TOKEN_CONTRACT));
            let mut cursor_value = None;
            let mut transfers = Vec::new();

            collect_matching_tron_transfer(
                &asset,
                transfer(Some(OTHER_TOKEN_CONTRACT), 50),
                &mut cursor_value,
                &mut transfers,
            );
            collect_matching_tron_transfer(
                &asset,
                transfer(Some(TOKEN_CONTRACT), 10),
                &mut cursor_value,
                &mut transfers,
            );

            assert_eq!(cursor_value, Some(10));
            assert_eq!(transfers.len(), 1);
            assert_eq!(transfers[0].1.cursor_value, 10);
        }

        #[test]
        fn trc20_cursor_keeps_highest_matching_transfer_cursor() {
            let asset = asset(Some(TOKEN_CONTRACT));
            let mut cursor_value = Some(10);
            let mut transfers = Vec::new();

            collect_matching_tron_transfer(
                &asset,
                transfer(Some(TOKEN_CONTRACT), 30),
                &mut cursor_value,
                &mut transfers,
            );
            collect_matching_tron_transfer(
                &asset,
                transfer(Some(TOKEN_CONTRACT), 20),
                &mut cursor_value,
                &mut transfers,
            );

            assert_eq!(cursor_value, Some(30));
            assert_eq!(transfers.len(), 2);
        }

        #[test]
        fn trc20_asset_skips_native_transfer() {
            assert!(!tron_transfer_should_scan(
                &asset(Some(TOKEN_CONTRACT)),
                &transfer(None, 1)
            ));
        }

        #[test]
        fn tron_paged_log_index_offsets_items_by_page() {
            assert_eq!(paged_log_index(0, 3).unwrap(), 3);
            assert_eq!(paged_log_index(1, 3).unwrap(), 10_003);
        }

        #[test]
        fn tron_page_limit_rejects_remaining_page_after_maximum() {
            let error = ensure_provider_page_limit(
                "TRON account transactions",
                MAX_PROVIDER_PAGES_PER_SCAN,
                true,
            )
            .unwrap_err();

            assert!(error.to_string().contains("TRON account transactions"));
            assert!(error.to_string().contains("pagination exceeded"));
        }

        #[test]
        fn cursor_constants_are_stable() {
            assert_eq!(TRON_TRX_TRANSFER_CURSOR, "tron_trx_transfer");
            assert_eq!(TRON_TRC20_TRANSFER_CURSOR, "tron_trc20_transfer");
        }
    }

    mod btc_worker_helpers {
        use chrono::{TimeZone, Utc};
        use coin_listener_chain_providers::btc::DecodedBtcTransfer;
        use coin_listener_core::models::ScanCursor;
        use uuid::Uuid;

        use crate::{
            btc_cursor_value, btc_scan_from_block, ensure_provider_page_limit, paged_log_index,
            should_process_btc_transaction_page, BTC_CURSOR_OVERLAP_BLOCKS, BTC_TRANSACTION_CURSOR,
            MAX_PROVIDER_PAGES_PER_SCAN,
        };

        #[test]
        fn btc_cursor_value_uses_highest_block_number() {
            let transfers = vec![transfer(800_000), transfer(800_003), transfer(800_001)];

            assert_eq!(btc_cursor_value(&transfers), Some(800_003));
        }

        #[test]
        fn btc_cursor_value_returns_none_for_empty_transfers() {
            assert_eq!(btc_cursor_value(&[]), None);
        }

        #[test]
        fn btc_cursor_constant_is_stable() {
            assert_eq!(BTC_TRANSACTION_CURSOR, "btc_transaction");
        }

        #[test]
        fn btc_scan_from_block_reprocesses_last_scanned_block_for_overlap() {
            let cursor = scan_cursor(800_000);

            assert_eq!(BTC_CURSOR_OVERLAP_BLOCKS, 1);
            assert_eq!(btc_scan_from_block(Some(&cursor)), 800_000);
        }

        #[test]
        fn btc_scan_from_block_clamps_overlap_to_zero() {
            let cursor = scan_cursor(0);

            assert_eq!(btc_scan_from_block(Some(&cursor)), 0);
            assert_eq!(btc_scan_from_block(None), 0);
        }

        #[test]
        fn provider_page_limit_errors_when_next_page_remains_after_maximum() {
            let error = ensure_provider_page_limit(
                "BTC address transactions",
                MAX_PROVIDER_PAGES_PER_SCAN,
                true,
            )
            .unwrap_err();

            assert!(error.to_string().contains("BTC address transactions"));
            assert!(error.to_string().contains("pagination exceeded"));
        }

        #[test]
        fn provider_page_limit_allows_last_page_at_maximum() {
            assert!(ensure_provider_page_limit(
                "BTC address transactions",
                MAX_PROVIDER_PAGES_PER_SCAN,
                false,
            )
            .is_ok());
        }

        #[test]
        fn btc_page_boundary_allows_tenth_non_empty_page() {
            assert!(
                should_process_btc_transaction_page(MAX_PROVIDER_PAGES_PER_SCAN - 1, 1).is_ok()
            );
        }

        #[test]
        fn btc_page_boundary_allows_empty_page_after_maximum() {
            assert!(should_process_btc_transaction_page(MAX_PROVIDER_PAGES_PER_SCAN, 0).is_ok());
        }

        #[test]
        fn btc_page_boundary_rejects_non_empty_page_after_maximum() {
            let error =
                should_process_btc_transaction_page(MAX_PROVIDER_PAGES_PER_SCAN, 1).unwrap_err();

            assert!(error.to_string().contains("BTC address transactions"));
            assert!(error.to_string().contains("pagination exceeded"));
        }

        #[test]
        fn paged_log_index_offsets_items_by_page() {
            assert_eq!(paged_log_index(0, 7).unwrap(), 7);
            assert_eq!(paged_log_index(2, 7).unwrap(), 20_007);
        }

        fn transfer(block_number: i64) -> DecodedBtcTransfer {
            DecodedBtcTransfer {
                tx_hash: format!("{block_number:064x}"),
                block_number,
                block_hash: None,
                direction: "in".to_string(),
                amount_raw: "1000".to_string(),
                amount_decimal: "0.00001".to_string(),
                received_raw: "1000".to_string(),
                spent_raw: "0".to_string(),
            }
        }

        fn scan_cursor(last_scanned_block: i64) -> ScanCursor {
            ScanCursor {
                id: Uuid::from_u128(1),
                tenant_id: Uuid::from_u128(2),
                chain_id: Uuid::from_u128(3),
                address_id: Uuid::from_u128(4),
                cursor_type: BTC_TRANSACTION_CURSOR.to_string(),
                last_scanned_block,
                updated_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap(),
            }
        }
    }

    #[test]
    fn process_locked_scan_task_receives_redis_for_provider_qps() {
        let source = include_str!("lib.rs");

        assert!(source.contains("process_locked_scan_task(pool, redis, &task, now).await"));
        assert!(source.contains("redis: &mut MultiplexedConnection"));
    }

    #[test]
    fn scan_entrypoints_receive_redis_for_provider_qps() {
        let source = include_str!("lib.rs");

        assert!(source.contains(
            "scan_evm_address(\n    pool: &PgPool,\n    redis: &mut MultiplexedConnection,"
        ));
        assert!(source.contains(
            "scan_tron_address(\n    pool: &PgPool,\n    redis: &mut MultiplexedConnection,"
        ));
        assert!(source.contains(
            "scan_btc_address(\n    pool: &PgPool,\n    redis: &mut MultiplexedConnection,"
        ));
    }

    #[test]
    fn worker_no_longer_enqueues_notify_tasks_after_scan() {
        let source = include_str!("lib.rs");
        let enqueue_call = ["notify_queue", "enqueue"].join(".");
        let notify_builder_call = format!("{}(event, now)", "build_notify_event_task");

        assert!(!source.contains(&enqueue_call));
        assert!(!source.contains(&notify_builder_call));
    }

    #[test]
    fn worker_event_insert_paths_use_outbox_helper() {
        let source = include_str!("lib.rs");
        let legacy_insert = format!("{}(pool, draft)", "insert_event_if_not_exists");
        let outbox_insert = format!("{}(pool, draft)", "insert_event_and_outbox_if_not_exists");

        assert!(!source.contains(&legacy_insert));
        assert!(source.matches(&outbox_insert).count() >= 4);
    }
}
