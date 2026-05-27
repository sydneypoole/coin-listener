use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use std::time::Duration as StdDuration;

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
        AddressEvent, Asset, BalanceSnapshot, CreateBalanceSnapshotRequest,
        CreateWatchedAddressRequest, Provider, ScanAddressContext, ScanAddressTask, ScanCursor,
    },
    AppError, AppResult,
};
use coin_listener_storage::{
    address_imports,
    provider_health::{
        active_rpc_provider_candidates, record_provider_failure, record_provider_success,
        try_acquire_provider_qps,
    },
    repositories,
    scan_jobs::{
        self, scan_job_next_attempt_at, scan_job_should_dead_letter, scan_job_to_task,
    },
    scan_queue::ScanQueue,
    scan_runs,
};
use redis::aio::MultiplexedConnection;
use sqlx::{PgPool, Postgres, Transaction};
use tracing::{info, warn};

pub const EVM_ERC20_TRANSFER_CURSOR: &str = "evm_erc20_transfer";
pub const EVM_TRANSFER_INITIAL_WINDOW_BLOCKS: i64 = 1_000;
pub const EVM_LOG_MAX_BLOCK_SPAN: i64 = EVM_TRANSFER_INITIAL_WINDOW_BLOCKS;
pub const TRON_TRX_TRANSFER_CURSOR: &str = "tron_trx_transfer";
pub const TRON_TRC20_TRANSFER_CURSOR: &str = "tron_trc20_transfer";
pub const BTC_TRANSACTION_CURSOR: &str = "btc_transaction";
pub const TRON_INITIAL_WATERMARK_WINDOW: i64 = 86_400_000;
pub const BTC_INITIAL_BLOCK_WINDOW: i64 = 3;
pub const MAX_PROVIDER_PAGES_PER_SCAN: usize = 10;
pub const BTC_CURSOR_OVERLAP_BLOCKS: i64 = 1;
pub const PROVIDER_PAGE_LOG_INDEX_STRIDE: usize = 10_000;
pub const ADDRESS_IMPORT_ROW_BATCH_SIZE: i64 = 50;

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
    Scanned { event_count: usize },
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

pub fn scan_task_outcome_log_status(outcome: &ScanTaskOutcome) -> &'static str {
    match outcome {
        ScanTaskOutcome::Locked => "locked",
        ScanTaskOutcome::Scanned { .. } => "success",
        ScanTaskOutcome::UnsupportedChain(_) => "unsupported",
    }
}

pub fn should_emit_balance_change(
    previous: Option<&BalanceSnapshot>,
    current: &BalanceSnapshot,
) -> bool {
    previous.is_some_and(|previous| previous.balance_raw != current.balance_raw)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRange {
    pub from_block: i64,
    pub to_block: i64,
}

pub fn bounded_block_ranges(
    from_block: i64,
    to_block: i64,
    max_block_span: i64,
) -> AppResult<Vec<BlockRange>> {
    if max_block_span <= 0 {
        return Err(AppError::Validation(
            "max block span must be positive".to_string(),
        ));
    }
    if to_block < from_block {
        return Ok(Vec::new());
    }

    let mut ranges = Vec::new();
    let mut current_from = from_block;
    while current_from <= to_block {
        let current_to = current_from
            .saturating_add(max_block_span - 1)
            .min(to_block);
        ranges.push(BlockRange {
            from_block: current_from,
            to_block: current_to,
        });
        if current_to == to_block {
            break;
        }
        current_from = current_to + 1;
    }
    Ok(ranges)
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

pub fn provider_timeout_duration(provider: &Provider) -> AppResult<StdDuration> {
    let timeout_ms = u64::try_from(provider.timeout_ms)
        .map_err(|_| AppError::Validation("timeout_ms must be positive".to_string()))?;
    if timeout_ms == 0 {
        return Err(AppError::Validation(
            "timeout_ms must be positive".to_string(),
        ));
    }
    Ok(StdDuration::from_millis(timeout_ms))
}

pub fn ensure_provider_matches_context(
    provider: &Provider,
    context: &ScanAddressContext,
) -> AppResult<()> {
    if provider.chain_id != context.chain_id {
        return Err(AppError::Validation(
            "provider chain_id does not match scan context".to_string(),
        ));
    }
    Ok(())
}

pub fn native_asset_selected(selected_assets: &[Asset], native_asset: &Asset) -> bool {
    selected_assets
        .iter()
        .any(|asset| asset.id == native_asset.id)
}

pub fn selected_assets_by_type(selected_assets: &[Asset], asset_type: &str) -> Vec<Asset> {
    let mut assets = selected_assets
        .iter()
        .filter(|asset| asset.asset_type == asset_type)
        .cloned()
        .collect::<Vec<_>>();
    assets.sort_by(|left, right| {
        left.symbol
            .cmp(&right.symbol)
            .then(left.name.cmp(&right.name))
    });
    assets
}

pub fn asset_type_selected(selected_assets: &[Asset], asset_type: &str) -> bool {
    selected_assets
        .iter()
        .any(|asset| asset.asset_type == asset_type)
}

pub fn provider_attempts_exhausted_error(
    chain_id: uuid::Uuid,
    last_provider_error: Option<AppError>,
) -> AppError {
    last_provider_error.unwrap_or_else(|| provider_capacity_error(chain_id))
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
    selected_assets: &[Asset],
) -> AppResult<Vec<AddressEvent>> {
    let cursor = repositories::scan_cursor(pool, context.id, EVM_ERC20_TRANSFER_CURSOR).await?;
    let Some((from_block, to_block)) =
        evm_transfer_scan_range(cursor.as_ref(), latest_block, default_confirmations)?
    else {
        return Ok(Vec::new());
    };

    let assets = selected_assets_by_type(selected_assets, "erc20");
    if assets.is_empty() {
        return Ok(Vec::new());
    }

    let watched_topic = address_to_topic(&context.address)?;
    let mut events = Vec::new();
    let ranges = bounded_block_ranges(from_block, to_block, EVM_LOG_MAX_BLOCK_SPAN)?;
    let mut last_successful_block = None;

    for range in ranges {
        for asset in &assets {
            let Some(contract_address) = asset.contract_address.clone() else {
                continue;
            };
            let incoming = EvmLogFilter {
                address: contract_address.clone(),
                from_block: range.from_block,
                to_block: range.to_block,
                topics: vec![
                    Some(TRANSFER_TOPIC0.to_string()),
                    None,
                    Some(watched_topic.clone()),
                ],
            };
            let outgoing = EvmLogFilter {
                address: contract_address,
                from_block: range.from_block,
                to_block: range.to_block,
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
                    let draft = transfer_event_draft(context, asset, transfer);
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
            range.to_block,
        )
        .await?;
        last_successful_block = Some(range.to_block);
    }

    if last_successful_block.is_none() {
        return Ok(events);
    }

    Ok(events)
}

pub async fn scan_evm_address_with_provider(
    pool: &PgPool,
    context: &ScanAddressContext,
    provider: &Provider,
) -> AppResult<Vec<AddressEvent>> {
    ensure_provider_matches_context(provider, context)?;
    let chain = repositories::chain_by_id(pool, context.chain_id).await?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let selected_assets = repositories::selected_assets_for_address(pool, context.id).await?;
    let timeout = provider_timeout_duration(provider)?;
    let rpc = EvmRpcClient::new(provider.base_url.clone(), timeout);
    let latest_block = rpc.eth_block_number().await?;

    let mut events = Vec::new();
    if native_asset_selected(&selected_assets, &native_asset) {
        if let Some(event) = scan_evm_native_balance_with_context(
            pool,
            &rpc,
            context,
            &native_asset,
            provider,
            latest_block,
        )
        .await?
        {
            events.push(event);
        }
    }
    events.extend(
        scan_evm_erc20_transfers(
            pool,
            &rpc,
            context,
            latest_block,
            chain.default_confirmations,
            &selected_assets,
        )
        .await?,
    );
    Ok(events)
}

pub async fn scan_evm_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    context: &ScanAddressContext,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let candidates = active_rpc_provider_candidates(pool, context.chain_id, now).await?;
    if candidates.is_empty() {
        return Err(provider_capacity_error(context.chain_id));
    }

    let mut last_provider_error = None;
    for candidate in candidates {
        let provider = candidate.provider;
        if !try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await? {
            info!(provider_id = %provider.id, chain_id = %context.chain_id, "provider qps limit reached");
            continue;
        }

        match scan_evm_address_with_provider(pool, &context, &provider).await {
            Ok(events) => {
                if let Err(error) = record_provider_success(pool, provider.id, now).await {
                    warn!(provider_id = %provider.id, error = %error, "failed to record provider success");
                }
                return Ok(events);
            }
            Err(error) if is_provider_availability_error(&error) => {
                if let Err(write_error) =
                    record_provider_failure(pool, provider.id, now, &error).await
                {
                    warn!(provider_id = %provider.id, error = %write_error, "failed to record provider failure");
                }
                last_provider_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(provider_attempts_exhausted_error(
        context.chain_id,
        last_provider_error,
    ))
}

pub async fn scan_tron_address_with_provider(
    pool: &PgPool,
    context: &ScanAddressContext,
    provider: &Provider,
) -> AppResult<Vec<AddressEvent>> {
    ensure_provider_matches_context(provider, context)?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let selected_assets = repositories::selected_assets_for_address(pool, context.id).await?;
    let timeout = provider_timeout_duration(provider)?;

    let client = TronClient::new(provider.base_url.clone(), timeout);

    let trx_cursor = repositories::scan_cursor(pool, context.id, TRON_TRX_TRANSFER_CURSOR).await?;
    let trx_from = trx_cursor
        .as_ref()
        .map(|cursor| cursor.last_scanned_block + 1)
        .unwrap_or(0);
    let mut trx_transfers = Vec::new();
    let mut trx_fingerprint: Option<String> = None;
    let mut trx_pages_processed = 0usize;

    if native_asset_selected(&selected_assets, &native_asset) {
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
    }

    let trx_cursor_value = tron_cursor_value(&trx_transfers);

    let trc20_assets = selected_assets_by_type(&selected_assets, "trc20");
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

pub async fn scan_tron_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    context: &ScanAddressContext,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let candidates = active_rpc_provider_candidates(pool, context.chain_id, now).await?;
    if candidates.is_empty() {
        return Err(provider_capacity_error(context.chain_id));
    }

    let mut last_provider_error = None;
    for candidate in candidates {
        let provider = candidate.provider;
        if !try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await? {
            info!(provider_id = %provider.id, chain_id = %context.chain_id, "provider qps limit reached");
            continue;
        }

        match scan_tron_address_with_provider(pool, &context, &provider).await {
            Ok(events) => {
                if let Err(error) = record_provider_success(pool, provider.id, now).await {
                    warn!(provider_id = %provider.id, error = %error, "failed to record provider success");
                }
                return Ok(events);
            }
            Err(error) if is_provider_availability_error(&error) => {
                if let Err(write_error) =
                    record_provider_failure(pool, provider.id, now, &error).await
                {
                    warn!(provider_id = %provider.id, error = %write_error, "failed to record provider failure");
                }
                last_provider_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(provider_attempts_exhausted_error(
        context.chain_id,
        last_provider_error,
    ))
}

pub async fn scan_btc_address_with_provider(
    pool: &PgPool,
    context: &ScanAddressContext,
    provider: &Provider,
) -> AppResult<Vec<AddressEvent>> {
    ensure_provider_matches_context(provider, context)?;
    let native_asset = repositories::native_asset_for_chain(pool, context.chain_id).await?;
    let selected_assets = repositories::selected_assets_for_address(pool, context.id).await?;
    if !native_asset_selected(&selected_assets, &native_asset) {
        return Ok(Vec::new());
    }
    let timeout = provider_timeout_duration(provider)?;

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

pub async fn scan_btc_address(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    context: &ScanAddressContext,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let candidates = active_rpc_provider_candidates(pool, context.chain_id, now).await?;
    if candidates.is_empty() {
        return Err(provider_capacity_error(context.chain_id));
    }

    let mut last_provider_error = None;
    for candidate in candidates {
        let provider = candidate.provider;
        if !try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await? {
            info!(provider_id = %provider.id, chain_id = %context.chain_id, "provider qps limit reached");
            continue;
        }

        match scan_btc_address_with_provider(pool, &context, &provider).await {
            Ok(events) => {
                if let Err(error) = record_provider_success(pool, provider.id, now).await {
                    warn!(provider_id = %provider.id, error = %error, "failed to record provider success");
                }
                return Ok(events);
            }
            Err(error) if is_provider_availability_error(&error) => {
                if let Err(write_error) =
                    record_provider_failure(pool, provider.id, now, &error).await
                {
                    warn!(provider_id = %provider.id, error = %write_error, "failed to record provider failure");
                }
                last_provider_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(provider_attempts_exhausted_error(
        context.chain_id,
        last_provider_error,
    ))
}

pub async fn process_scan_task(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
    process_locked_scan_task(pool, redis, &task, now).await
}

pub fn scan_task_event_count(outcome: &ScanTaskOutcome) -> i32 {
    match outcome {
        ScanTaskOutcome::Scanned { event_count } => (*event_count).min(i32::MAX as usize) as i32,
        ScanTaskOutcome::Locked | ScanTaskOutcome::UnsupportedChain(_) => 0,
    }
}

pub fn scan_task_update_metadata(outcome: &ScanTaskOutcome) -> serde_json::Value {
    match outcome {
        ScanTaskOutcome::Scanned { .. } => serde_json::json!({ "outcome": "scanned" }),
        ScanTaskOutcome::Locked => serde_json::json!({ "outcome": "locked" }),
        ScanTaskOutcome::UnsupportedChain(chain_type) => {
            serde_json::json!({ "outcome": "unsupported", "chain_type": chain_type })
        }
    }
}

async fn create_scan_run_for_task(
    pool: &PgPool,
    task: &ScanAddressTask,
    started_at: DateTime<Utc>,
    worker_id: &str,
) -> AppResult<coin_listener_core::models::ScanRun> {
    let context = scan_runs::scan_run_context(pool, task).await?;
    scan_runs::create_scan_run(
        pool,
        task,
        &context,
        worker_id,
        started_at,
        serde_json::json!({
            "worker_id": worker_id,
            "scan_job_id": task.task_id,
            "attempt": task.attempt,
        }),
    )
    .await
}

async fn finish_scan_run_for_result(
    pool: &PgPool,
    scan_run_id: uuid::Uuid,
    result: &AppResult<ScanTaskOutcome>,
    finished_at: DateTime<Utc>,
) -> AppResult<()> {
    finish_scan_run_for_result_with_executor(pool, scan_run_id, result, finished_at).await
}

async fn finish_scan_run_for_result_with_executor<'e, E>(
    executor: E,
    scan_run_id: uuid::Uuid,
    result: &AppResult<ScanTaskOutcome>,
    finished_at: DateTime<Utc>,
) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    match result {
        Ok(outcome) => {
            scan_runs::finish_scan_run_with_executor(
                executor,
                scan_run_id,
                scan_task_outcome_log_status(outcome),
                scan_task_event_count(outcome),
                finished_at,
                None,
                scan_task_update_metadata(outcome),
            )
            .await?;
        }
        Err(error) => {
            let error_message = error.to_string();
            scan_runs::finish_scan_run_with_executor(
                executor,
                scan_run_id,
                scan_runs::SCAN_RUN_STATUS_FAILED,
                0,
                finished_at,
                Some(error_message.as_str()),
                serde_json::json!({ "outcome": "failed" }),
            )
            .await?;
        }
    }
    Ok(())
}

fn lease_renewal_interval_seconds(lease_ttl_seconds: u64) -> u64 {
    ((lease_ttl_seconds as u64) / 3).max(1)
}

fn spawn_scan_job_lease_renewal(
    pool: PgPool,
    job_id: uuid::Uuid,
    worker_id: String,
    lease_ttl_seconds: u64,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    ownership_lost: tokio::sync::watch::Sender<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(StdDuration::from_secs(
            lease_renewal_interval_seconds(lease_ttl_seconds),
        ));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                    continue;
                }
            }
            if *shutdown.borrow() {
                break;
            }
            match scan_jobs::renew_scan_job_lease(
                &pool,
                job_id,
                &worker_id,
                lease_ttl_seconds,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    let _ = ownership_lost.send(true);
                    break;
                }
                Err(error) => {
                    warn!(job_id = %job_id, error = %error, "failed to renew scan job lease");
                    let _ = ownership_lost.send(true);
                    break;
                }
            }
        }
    })
}

async fn finish_scan_job_for_result(
    pool: &PgPool,
    job_id: uuid::Uuid,
    worker_id: &str,
    attempt_count: i32,
    max_attempts: i32,
    scan_run_id: Option<uuid::Uuid>,
    result: &AppResult<ScanTaskOutcome>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    match result {
        Ok(_) => scan_jobs::mark_scan_job_succeeded(pool, job_id, worker_id, scan_run_id).await,
        Err(error) => {
            let error_message = error.to_string();
            if scan_job_should_dead_letter(attempt_count, max_attempts) {
                scan_jobs::mark_scan_job_dead_letter(
                    pool,
                    job_id,
                    worker_id,
                    &error_message,
                    scan_run_id,
                )
                .await
            } else {
                scan_jobs::mark_scan_job_retryable(
                    pool,
                    job_id,
                    worker_id,
                    scan_job_next_attempt_at(now, attempt_count),
                    &error_message,
                    scan_run_id,
                )
                .await
            }
        }
    }
}

async fn finish_scan_job_for_result_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: uuid::Uuid,
    worker_id: &str,
    attempt_count: i32,
    max_attempts: i32,
    scan_run_id: uuid::Uuid,
    result: &AppResult<ScanTaskOutcome>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    match result {
        Ok(_) => {
            scan_jobs::mark_scan_job_succeeded_with_executor(
                transaction.as_mut(),
                job_id,
                worker_id,
                Some(scan_run_id),
            )
            .await
        }
        Err(error) => {
            let error_message = error.to_string();
            if scan_job_should_dead_letter(attempt_count, max_attempts) {
                scan_jobs::mark_scan_job_dead_letter_with_executor(
                    transaction.as_mut(),
                    job_id,
                    worker_id,
                    &error_message,
                    Some(scan_run_id),
                )
                .await
            } else {
                scan_jobs::mark_scan_job_retryable_with_executor(
                    transaction.as_mut(),
                    job_id,
                    worker_id,
                    scan_job_next_attempt_at(now, attempt_count),
                    &error_message,
                    Some(scan_run_id),
                )
                .await
            }
        }
    }
}

async fn finish_scan_run_for_result_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    scan_run_id: uuid::Uuid,
    result: &AppResult<ScanTaskOutcome>,
    finished_at: DateTime<Utc>,
) -> AppResult<()> {
    finish_scan_run_for_result_with_executor(
        transaction.as_mut(),
        scan_run_id,
        result,
        finished_at,
    )
    .await
}

async fn finish_scan_job_and_run_for_result(
    pool: &PgPool,
    job_id: uuid::Uuid,
    worker_id: &str,
    attempt_count: i32,
    max_attempts: i32,
    scan_run_id: Option<uuid::Uuid>,
    result: &AppResult<ScanTaskOutcome>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let Some(scan_run_id) = scan_run_id else {
        return finish_scan_job_for_result(
            pool,
            job_id,
            worker_id,
            attempt_count,
            max_attempts,
            None,
            result,
            now,
        )
        .await;
    };

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
    finish_scan_job_for_result_in_transaction(
        &mut transaction,
        job_id,
        worker_id,
        attempt_count,
        max_attempts,
        scan_run_id,
        result,
        now,
    )
    .await?;
    finish_scan_run_for_result_in_transaction(&mut transaction, scan_run_id, result, now).await?;
    transaction
        .commit()
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn process_one_address_import_task(
    pool: &PgPool,
    worker_id: &str,
    now: DateTime<Utc>,
) -> AppResult<bool> {
    let Some(task) =
        address_imports::claim_next_watched_address_import(pool, now, worker_id).await?
    else {
        return Ok(false);
    };

    address_imports::renew_watched_address_import_lock(
        pool,
        task.tenant_id,
        task.id,
        worker_id,
        now,
    )
    .await?;
    let attempts = address_imports::pending_import_attempts(
        pool,
        task.tenant_id,
        task.id,
        worker_id,
        ADDRESS_IMPORT_ROW_BATCH_SIZE,
    )
    .await?;

    for attempt in attempts {
        address_imports::renew_watched_address_import_lock(
            pool,
            task.tenant_id,
            task.id,
            worker_id,
            Utc::now(),
        )
        .await?;
        let address_text = attempt.address.clone();
        let request = CreateWatchedAddressRequest {
            tenant_id: Some(task.tenant_id),
            chain_id: attempt.chain_id,
            address: address_text.clone(),
            label: attempt.label,
            priority: attempt.priority.unwrap_or_else(|| task.priority.clone()),
            scan_interval_seconds: attempt
                .scan_interval_seconds
                .unwrap_or(task.scan_interval_seconds),
            transfer_filter_enabled: attempt
                .transfer_filter_enabled
                .unwrap_or(task.transfer_filter_enabled),
            balance_change_filter_enabled: attempt
                .balance_change_filter_enabled
                .unwrap_or(task.balance_change_filter_enabled),
            status: attempt
                .status
                .unwrap_or_else(|| task.address_status.clone()),
            asset_ids: attempt.asset_ids.clone(),
        };

        match address_imports::create_watched_address_for_import_attempt(
            pool,
            task.tenant_id,
            task.id,
            worker_id,
            attempt.attempt_id,
            request,
        )
        .await
        {
            Ok(_) => {}
            Err(error) => {
                address_imports::mark_import_attempt_failed_with_lock(
                    pool,
                    task.tenant_id,
                    attempt.attempt_id,
                    Some(task.id),
                    Some(worker_id),
                    "create_failed",
                    &error.to_string(),
                )
                .await?;
            }
        }
    }

    address_imports::refresh_import_task_counts(pool, task.tenant_id, task.id).await?;
    address_imports::complete_import_if_finished(pool, task.tenant_id, task.id, now).await?;
    Ok(true)
}

async fn process_locked_scan_task(
    pool: &PgPool,
    redis: &mut MultiplexedConnection,
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<ScanTaskOutcome> {
    let context = scan_runs::scan_run_context(pool, task).await?;
    let next_scan_at = repositories::next_scan_at_from(now, context.scan_interval_seconds);

    match scan_plan_for_chain(&context.chain_type) {
        ScanPlan::EvmNativeBalance => {
            let events = scan_evm_address(pool, redis, &context, now).await?;
            repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
            Ok(ScanTaskOutcome::Scanned {
                event_count: events.len(),
            })
        }
        ScanPlan::Tron => {
            let events = scan_tron_address(pool, redis, &context, now).await?;
            repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
            Ok(ScanTaskOutcome::Scanned {
                event_count: events.len(),
            })
        }
        ScanPlan::Btc => {
            let events = scan_btc_address(pool, redis, &context, now).await?;
            repositories::finish_address_scan(pool, task.address_id, now, next_scan_at).await?;
            Ok(ScanTaskOutcome::Scanned {
                event_count: events.len(),
            })
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
    worker_id: String,
    job_idle_sleep_ms: u64,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
    while !worker_shutdown_requested(&shutdown) {
        if let Err(error) = process_one_address_import_task(&pool, &worker_id, Utc::now()).await {
            warn!(error = %error, "address import task processing failed");
        }

        match scan_jobs::claim_due_scan_job(
            &pool,
            &worker_id,
            scan_queue.lock_ttl_seconds(),
        )
        .await
        {
            Ok(Some(job)) => {
                let task = scan_job_to_task(&job, Utc::now());
                let task_id = task.task_id;
                let address_id = task.address_id;
                let started_at = Utc::now();
                let (renewal_shutdown, renewal_shutdown_rx) = tokio::sync::watch::channel(false);
                let (renewal_lost_tx, mut renewal_lost_rx) = tokio::sync::watch::channel(false);
                let renewal_handle = spawn_scan_job_lease_renewal(
                    pool.clone(),
                    task_id,
                    worker_id.clone(),
                    scan_queue.lock_ttl_seconds(),
                    renewal_shutdown_rx,
                    renewal_lost_tx,
                );
                let scan_run_result = create_scan_run_for_task(&pool, &task, started_at, &worker_id).await;
                let mut audit_allows_scan = true;
                let mut scan_blocker = None;
                let scan_run = match scan_run_result {
                    Ok(run) => Some(run),
                    Err(error) => {
                        audit_allows_scan = false;
                        scan_blocker = Some(error);
                        warn!(
                            task_id = %task_id,
                            address_id = %address_id,
                            error = %scan_blocker.as_ref().expect("scan blocker just set"),
                            "failed to create scan run audit"
                        );
                        None
                    }
                };
                let scan_run_id = scan_run.as_ref().map(|run| run.id);
                let result = if audit_allows_scan && !*renewal_lost_rx.borrow() {
                    tokio::select! {
                        result = process_scan_task(&pool, &mut redis, task, started_at) => result,
                        changed = renewal_lost_rx.changed() => {
                            if changed.is_ok() && *renewal_lost_rx.borrow() {
                                Err(AppError::Database("scan job lease ownership lost".to_string()))
                            } else {
                                Err(AppError::Database("scan job lease renewal monitor closed".to_string()))
                            }
                        }
                    }
                } else {
                    Err(scan_blocker.unwrap_or_else(|| {
                        AppError::Database("scan job lease ownership lost before scan side effects".to_string())
                    }))
                };

                let mut job_finalized = true;
                let mut job_finalization_error = None;
                if let Err(error) = finish_scan_job_and_run_for_result(
                    &pool,
                    task_id,
                    &worker_id,
                    job.attempt_count,
                    job.max_attempts,
                    scan_run_id,
                    &result,
                    Utc::now(),
                )
                .await
                {
                    job_finalized = false;
                    job_finalization_error = Some(error.to_string());
                    warn!(
                        task_id = %task_id,
                        address_id = %address_id,
                        scan_run_id = ?scan_run_id,
                        error = %error,
                        "failed to update scan job state"
                    );
                }

                if !job_finalized {
                    if let Some(scan_run_id) = scan_run_id {
                        let audit_error = AppError::Database(format!(
                            "scan job finalization failed: {}",
                            job_finalization_error
                                .as_deref()
                                .unwrap_or("unknown scan job finalization error")
                        ));
                        let result_for_audit: AppResult<ScanTaskOutcome> = Err(audit_error);
                        if let Err(error) =
                            finish_scan_run_for_result(&pool, scan_run_id, &result_for_audit, Utc::now()).await
                        {
                            warn!(
                                task_id = %task_id,
                                address_id = %address_id,
                                scan_run_id = %scan_run_id,
                                error = %error,
                                "failed to update scan run audit"
                            );
                        }
                    }
                }

                let _ = renewal_shutdown.send(true);
                if let Err(error) = renewal_handle.await {
                    warn!(task_id = %task_id, error = %error, "scan job lease renewal task failed");
                }

                match result {
                    Ok(outcome) => info!(
                        task_id = %task_id,
                        address_id = %address_id,
                        scan_run_id = ?scan_run_id,
                        scan_status = scan_task_outcome_log_status(&outcome),
                        ?outcome,
                        "scan task processed"
                    ),
                    Err(error) => warn!(
                        task_id = %task_id,
                        address_id = %address_id,
                        scan_run_id = ?scan_run_id,
                        scan_status = "failed",
                        error = %error,
                        "scan task failed"
                    ),
                }
            }
            Ok(None) => {
                if let Err(error) = scan_queue.wait_for_signal(&mut redis, 5).await {
                    warn!(error = %error, "scan wake signal wait failed");
                    tokio::time::sleep(StdDuration::from_millis(job_idle_sleep_ms)).await;
                }
            }
            Err(error) => {
                warn!(error = %error, "failed to claim durable scan job");
                tokio::time::sleep(StdDuration::from_millis(job_idle_sleep_ms)).await;
            }
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

    mod scan_task_logging {
        use crate::{
            scan_task_event_count, scan_task_outcome_log_status, scan_task_update_metadata,
            ScanTaskOutcome,
        };

        #[test]
        fn scan_outcomes_have_explicit_log_statuses() {
            assert_eq!(
                scan_task_outcome_log_status(&ScanTaskOutcome::Scanned { event_count: 2 }),
                "success"
            );
            assert_eq!(
                scan_task_outcome_log_status(&ScanTaskOutcome::Locked),
                "locked"
            );
            assert_eq!(
                scan_task_outcome_log_status(&ScanTaskOutcome::UnsupportedChain(
                    "solana".to_string()
                )),
                "unsupported"
            );
        }

        #[test]
        fn scan_outcomes_expose_event_counts_for_audit() {
            assert_eq!(
                scan_task_event_count(&ScanTaskOutcome::Scanned { event_count: 2 }),
                2
            );
            assert_eq!(scan_task_event_count(&ScanTaskOutcome::Locked), 0);
            assert_eq!(
                scan_task_event_count(&ScanTaskOutcome::UnsupportedChain("solana".to_string())),
                0
            );
        }

        #[test]
        fn scan_outcome_metadata_records_audit_outcome() {
            assert_eq!(
                scan_task_update_metadata(&ScanTaskOutcome::Scanned { event_count: 2 })["outcome"],
                "scanned"
            );
            assert_eq!(
                scan_task_update_metadata(&ScanTaskOutcome::Locked)["outcome"],
                "locked"
            );
            assert_eq!(
                scan_task_update_metadata(&ScanTaskOutcome::UnsupportedChain("solana".to_string()))
                    ["chain_type"],
                "solana"
            );
        }

        #[test]
        fn worker_logs_explicit_scan_success_and_failure_status() {
            let source = include_str!("lib.rs");
            let start = source
                .find("pub async fn run_worker(")
                .expect("worker loop exists");
            let end = source[start..]
                .find("\n#[cfg(test)]")
                .expect("test module marker")
                + start;
            let worker = &source[start..end];

            assert!(worker.contains("scan_status = scan_task_outcome_log_status(&outcome)"));
            assert!(worker.contains("scan_status = \"failed\""));
            assert!(worker.contains("scan_run_id = ?scan_run_id"));
            assert!(worker.contains("\"scan task processed\""));
            assert!(worker.contains("\"scan task failed\""));
            assert!(!worker.contains("\"scan task succeeded\""));
        }

        #[test]
        fn worker_creates_and_finishes_scan_run_audit_records() {
            let source = include_str!("lib.rs");
            let start = source
                .find("pub async fn run_worker(")
                .expect("worker loop exists");
            let end = source[start..]
                .find("\n#[cfg(test)]")
                .expect("test module marker")
                + start;
            let worker = &source[start..end];

            assert!(worker.contains("create_scan_run_for_task"));
            assert!(worker.contains("finish_scan_run_for_result"));
            assert!(worker.contains("failed to create scan run audit"));
            assert!(worker.contains("failed to update scan run audit"));
        }

        #[test]
        fn process_locked_scan_task_reports_inserted_event_count() {
            let source = include_str!("lib.rs");
            let start = source
                .find("async fn process_locked_scan_task")
                .expect("process_locked_scan_task exists");
            let end = source[start..]
                .find("pub async fn run_worker")
                .expect("run_worker follows process_locked_scan_task")
                + start;
            let function = &source[start..end];

            assert!(function.contains("let events = scan_evm_address"));
            assert!(function.contains("let events = scan_tron_address"));
            assert!(function.contains("let events = scan_btc_address"));
            assert!(function.contains("ScanTaskOutcome::Scanned {"));
            assert!(function.contains("event_count: events.len()"));
        }

        #[test]
        fn process_locked_scan_task_loads_context_by_task_scope() {
            let source = include_str!("lib.rs");
            let start = source
                .find("async fn process_locked_scan_task")
                .expect("process_locked_scan_task exists");
            let end = source[start..]
                .find("pub async fn run_worker")
                .expect("run_worker follows process_locked_scan_task")
                + start;
            let function = &source[start..end];

            assert!(function.contains("scan_runs::scan_run_context(pool, task).await?"));
            assert!(!function
                .contains("repositories::get_scan_address_context(pool, task.address_id).await?"));
        }

        #[test]
        fn process_locked_scan_task_passes_validated_context_to_scan_entrypoints() {
            let source = include_str!("lib.rs");
            let start = source
                .find("async fn process_locked_scan_task")
                .expect("process_locked_scan_task exists");
            let end = source[start..]
                .find("pub async fn run_worker")
                .expect("run_worker follows process_locked_scan_task")
                + start;
            let function = &source[start..end];

            assert!(function.contains("let events = scan_evm_address(pool, redis, &context, now).await?;"));
            assert!(function.contains("let events = scan_tron_address(pool, redis, &context, now).await?;"));
            assert!(function.contains("let events = scan_btc_address(pool, redis, &context, now).await?;"));
        }
    }

    mod provider_failover_helpers {
        use coin_listener_core::{
            models::{Provider, ScanAddressContext},
            AppError,
        };
        use uuid::Uuid;

        use crate::{
            ensure_provider_matches_context, is_provider_availability_error,
            provider_attempts_exhausted_error, provider_capacity_error, provider_timeout_duration,
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
        fn provider_attempts_exhausted_prefers_last_provider_error() {
            let error = provider_attempts_exhausted_error(
                Uuid::from_u128(7),
                Some(AppError::Config(
                    "provider request failed: timeout".to_string(),
                )),
            );

            assert!(
                matches!(error, AppError::Config(message) if message == "provider request failed: timeout")
            );
        }

        #[test]
        fn provider_attempts_exhausted_returns_capacity_without_provider_error() {
            let error = provider_attempts_exhausted_error(Uuid::from_u128(7), None);

            assert!(
                matches!(error, AppError::Config(message) if message.contains("no active rpc provider capacity for chain 00000000-0000-0000-0000-000000000007"))
            );
        }

        #[test]
        fn provider_context_validation_rejects_mismatched_chains() {
            let provider = provider_with_timeout(1500);
            let context = scan_context(Uuid::from_u128(3));

            let error = ensure_provider_matches_context(&provider, &context).unwrap_err();

            assert!(
                matches!(error, AppError::Validation(message) if message == "provider chain_id does not match scan context")
            );
        }

        #[test]
        fn provider_context_validation_accepts_matching_chains() {
            let provider = provider_with_timeout(1500);
            let context = scan_context(provider.chain_id);

            assert!(ensure_provider_matches_context(&provider, &context).is_ok());
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

        fn scan_context(chain_id: Uuid) -> ScanAddressContext {
            ScanAddressContext {
                id: Uuid::from_u128(10),
                tenant_id: Uuid::from_u128(11),
                chain_id,
                address: "0x0000000000000000000000000000000000000000".to_string(),
                scan_interval_seconds: 60,
                chain_type: "evm".to_string(),
            }
        }
    }

    mod selected_asset_filters {
        use coin_listener_core::models::Asset;
        use uuid::Uuid;

        use crate::{asset_type_selected, native_asset_selected, selected_assets_by_type};

        fn asset(
            id: u128,
            asset_type: &str,
            symbol: &str,
            contract_address: Option<&str>,
        ) -> Asset {
            Asset {
                id: Uuid::from_u128(id),
                chain_id: Uuid::from_u128(2),
                asset_type: asset_type.to_string(),
                symbol: symbol.to_string(),
                name: symbol.to_string(),
                contract_address: contract_address.map(ToString::to_string),
                decimals: 18,
                is_builtin: true,
                status: "active".to_string(),
            }
        }

        #[test]
        fn native_asset_selected_only_when_native_id_is_present() {
            let native = asset(1, "native", "ETH", None);
            let usdt = asset(
                2,
                "erc20",
                "USDT",
                Some("0xdAC17F958D2ee523a2206206994597C13D831ec7"),
            );

            assert!(native_asset_selected(
                &[native.clone(), usdt.clone()],
                &native
            ));
            assert!(!native_asset_selected(&[usdt], &native));
        }

        #[test]
        fn selected_assets_by_type_filters_contract_assets() {
            let eth = asset(1, "native", "ETH", None);
            let usdt = asset(
                2,
                "erc20",
                "USDT",
                Some("0xdAC17F958D2ee523a2206206994597C13D831ec7"),
            );
            let usdc = asset(
                3,
                "erc20",
                "USDC",
                Some("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
            );

            let selected = selected_assets_by_type(&[eth, usdt.clone(), usdc.clone()], "erc20");

            assert_eq!(selected, vec![usdc, usdt]);
        }

        #[test]
        fn asset_type_selected_detects_any_selected_asset_of_type() {
            let btc = asset(1, "native", "BTC", None);
            assert!(asset_type_selected(&[btc], "native"));
            assert!(!asset_type_selected(&[], "native"));
        }
    }

    mod worker_shutdown_requested {
        use std::sync::atomic::AtomicBool;

        use crate::worker_shutdown_requested;

        #[test]
        fn set_shutdown_flag_stops_before_next_claim() {
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

        use crate::{
            bounded_block_ranges, evm_transfer_scan_range, BlockRange, EVM_ERC20_TRANSFER_CURSOR,
            EVM_LOG_MAX_BLOCK_SPAN, EVM_TRANSFER_INITIAL_WINDOW_BLOCKS,
        };

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

        #[test]
        fn bounded_block_ranges_split_large_inclusive_ranges() {
            let ranges = bounded_block_ranges(1, 25_000, 10_000).unwrap();

            assert_eq!(
                ranges,
                vec![
                    BlockRange {
                        from_block: 1,
                        to_block: 10_000,
                    },
                    BlockRange {
                        from_block: 10_001,
                        to_block: 20_000,
                    },
                    BlockRange {
                        from_block: 20_001,
                        to_block: 25_000,
                    },
                ]
            );
        }

        #[test]
        fn bounded_block_ranges_keep_exact_limit_as_one_range() {
            let ranges = bounded_block_ranges(5, 1_004, EVM_LOG_MAX_BLOCK_SPAN).unwrap();

            assert_eq!(
                ranges,
                vec![BlockRange {
                    from_block: 5,
                    to_block: 1_004,
                }]
            );
        }

        #[test]
        fn evm_log_max_block_span_matches_initial_window() {
            assert_eq!(EVM_LOG_MAX_BLOCK_SPAN, EVM_TRANSFER_INITIAL_WINDOW_BLOCKS);
        }

        #[test]
        fn bounded_block_ranges_terminates_at_i64_max() {
            let ranges = bounded_block_ranges(i64::MAX - 2, i64::MAX, 10).unwrap();

            assert_eq!(
                ranges,
                vec![BlockRange {
                    from_block: i64::MAX - 2,
                    to_block: i64::MAX,
                }]
            );
        }

        #[test]
        fn bounded_block_ranges_reject_non_positive_span() {
            let error = bounded_block_ranges(1, 10, 0).unwrap_err();

            assert!(error
                .to_string()
                .contains("max block span must be positive"));
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

    fn address_import_worker_source() -> &'static str {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn process_one_address_import_task")
            .expect("address import worker function");
        let end = source[start..]
            .find("async fn process_locked_scan_task")
            .expect("next worker function")
            + start;
        &source[start..end]
    }

    #[test]
    fn address_import_batch_size_is_bounded() {
        assert_eq!(crate::ADDRESS_IMPORT_ROW_BATCH_SIZE, 50);
    }

    #[test]
    fn address_import_worker_processes_attempt_chain_configs() {
        let body = address_import_worker_source();

        assert!(body.contains("pending_import_attempts"));
        assert!(body.contains("chain_id: attempt.chain_id"));
        assert!(body.contains("asset_ids: attempt.asset_ids.clone()"));
        assert!(body.contains("create_watched_address_for_import_attempt"));
        assert!(body.contains("task.id"));
        assert!(body.contains("worker_id"));
        assert!(body.contains("mark_import_attempt_failed_with_lock"));
        assert!(body.contains("Some(task.id)"));
        assert!(body.contains("Some(worker_id)"));
        assert!(!body.contains("chain_id: task.chain_id"));
        assert!(!body.contains("asset_ids: task.asset_ids.clone()"));
    }

    #[test]
    fn address_import_worker_marks_attempts_by_attempt_id() {
        let body = address_import_worker_source();

        assert!(body.contains("attempt.attempt_id"));
        assert!(!body.contains("row.row_number"));
    }

    #[test]
    fn address_import_worker_does_not_call_row_import_apis() {
        let body = address_import_worker_source();

        for row_api in [
            "pending_import_rows",
            "mark_import_row_success",
            "mark_import_row_failed",
        ] {
            assert!(!body.contains(row_api), "worker still calls {row_api}");
        }
    }

    #[test]
    fn address_import_worker_renews_task_lock_before_attempt_batch() {
        let body = address_import_worker_source();
        let renew_index = body.find("renew_watched_address_import_lock").unwrap();
        let attempts_index = body.find("pending_import_attempts").unwrap();

        assert!(renew_index < attempts_index);
    }

    #[test]
    fn address_import_worker_renews_task_lock_during_attempt_batch() {
        let body = address_import_worker_source();

        assert!(body.matches("renew_watched_address_import_lock").count() >= 2);
    }

    #[test]
    fn address_import_worker_does_not_treat_duplicate_create_as_success() {
        let body = address_import_worker_source();

        assert!(!body.contains("duplicate key value violates unique constraint"));
        assert!(!body.contains("imported_watched_address"));
    }

    #[test]
    fn run_worker_uses_caller_supplied_worker_id_for_import_locks() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let run_worker_source = &source[run_worker_index..];

        assert!(run_worker_source.contains("worker_id: String"));
        assert!(run_worker_source.contains("process_one_address_import_task(&pool, &worker_id"));
        assert!(!run_worker_source.contains("process_one_address_import_task(&pool, \"worker\""));
    }

    #[test]
    fn worker_source_processes_address_import_before_scan_job_claim() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let tests_index = source.find("#[cfg(test)]").unwrap();
        let run_worker_source = &source[run_worker_index..tests_index];
        let import_index = run_worker_source
            .find("process_one_address_import_task(&pool")
            .unwrap();
        let claim_index = run_worker_source
            .find("scan_jobs::claim_due_scan_job")
            .unwrap();

        assert!(import_index < claim_index);
        assert!(!run_worker_source.contains("scan_queue.dequeue"));
    }

    #[test]
    fn scan_job_lease_renewal_shutdown_is_wakeup_based() {
        let source = include_str!("lib.rs");
        let start = source.find("fn spawn_scan_job_lease_renewal(").unwrap();
        let end = source[start..]
            .find("async fn finish_scan_job_for_result")
            .unwrap()
            + start;
        let renewal_source = &source[start..end];

        assert!(renewal_source.contains("tokio::sync::watch::Receiver<bool>"));
        assert!(renewal_source.contains("tokio::select!"));
        assert!(renewal_source.contains("shutdown.changed()"));
    }

    #[test]
    fn scan_job_claim_errors_back_off_before_next_loop() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let tests_index = source.find("#[cfg(test)]").unwrap();
        let run_worker_source = &source[run_worker_index..tests_index];
        let error_index = run_worker_source
            .find("failed to claim durable scan job")
            .unwrap();
        let after_error = &run_worker_source[error_index..];

        assert!(after_error.contains("tokio::time::sleep(StdDuration::from_millis(job_idle_sleep_ms)).await"));
    }

    #[test]
    fn scan_job_lease_renewal_covers_final_job_state_update() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let tests_index = source.find("#[cfg(test)]").unwrap();
        let run_worker_source = &source[run_worker_index..tests_index];
        let finish_index = run_worker_source
            .find("finish_scan_job_and_run_for_result(")
            .unwrap();
        let shutdown_index = run_worker_source.find("renewal_shutdown.send(true)").unwrap();

        assert!(finish_index < shutdown_index);
    }

    #[test]
    fn scan_job_lease_renewal_starts_before_scan_run_creation() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let tests_index = source.find("#[cfg(test)]").unwrap();
        let run_worker_source = &source[run_worker_index..tests_index];
        let renewal_index = run_worker_source
            .find("spawn_scan_job_lease_renewal(")
            .unwrap();
        let scan_run_index = run_worker_source.find("create_scan_run_for_task(").unwrap();

        assert!(renewal_index < scan_run_index);
    }

    #[test]
    fn scan_run_creation_uses_db_owner_guard_without_cancelling_insert() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let tests_index = source.find("#[cfg(test)]").unwrap();
        let run_worker_source = &source[run_worker_index..tests_index];
        let scan_run_index = run_worker_source.find("let scan_run_result =").unwrap();
        let scan_run_block = &run_worker_source[scan_run_index..];
        let scan_run_match_index = scan_run_block.find("let scan_run = match scan_run_result").unwrap();
        let scan_run_creation = &scan_run_block[..scan_run_match_index];

        assert!(scan_run_creation.contains(
            "let scan_run_result = create_scan_run_for_task(&pool, &task, started_at, &worker_id).await;"
        ));
        assert!(!scan_run_creation.contains("tokio::select!"));
        assert!(scan_run_block.contains("audit_allows_scan = false"));
        assert!(scan_run_block.contains("if audit_allows_scan && !*renewal_lost_rx.borrow()"));
    }

    #[test]
    fn scan_run_creation_failure_blocks_scan_side_effects() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let tests_index = source.find("#[cfg(test)]").unwrap();
        let run_worker_source = &source[run_worker_index..tests_index];
        let scan_run_index = run_worker_source.find("let scan_run = match scan_run_result").unwrap();
        let scan_run_block = &run_worker_source[scan_run_index..];
        let process_index = scan_run_block.find("process_scan_task(").unwrap();
        let before_process = &scan_run_block[..process_index];

        assert!(before_process.contains("scan_blocker = Some(error)"));
        assert!(before_process.contains("audit_allows_scan = false"));
        assert!(!before_process.contains("if *renewal_lost_rx.borrow()"));
    }

    #[test]
    fn scan_job_and_scan_run_success_finalize_in_one_transaction() {
        let source = include_str!("lib.rs");
        let start = source.find("async fn finish_scan_job_and_run_for_result").unwrap();
        let end = source[start..]
            .find("pub async fn process_one_address_import_task")
            .unwrap()
            + start;
        let function = &source[start..end];
        let begin_index = function.find(".begin()").unwrap();
        let job_index = function.find("finish_scan_job_for_result_in_transaction").unwrap();
        let run_index = function.find("finish_scan_run_for_result_in_transaction").unwrap();
        let commit_index = function.find(".commit()").unwrap();

        assert!(begin_index < job_index);
        assert!(job_index < run_index);
        assert!(run_index < commit_index);
    }

    #[test]
    fn scan_run_audit_fallback_only_runs_when_scan_job_finalization_fails() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let tests_index = source.find("#[cfg(test)]").unwrap();
        let run_worker_source = &source[run_worker_index..tests_index];
        let finalization_index = run_worker_source.find("finish_scan_job_and_run_for_result(").unwrap();
        let after_finalization = &run_worker_source[finalization_index..];
        let finish_run_index = after_finalization.find("finish_scan_run_for_result(").unwrap();
        let before_finish_run = &after_finalization[..finish_run_index];

        assert!(before_finish_run.contains("job_finalization_error = Some(error.to_string())"));
        assert!(before_finish_run.contains("if !job_finalized"));
        assert!(!before_finish_run.contains("let result_for_audit = if job_finalized"));
    }

    #[test]
    fn scan_job_renewal_loss_aborts_scan_side_effects() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let tests_index = source.find("#[cfg(test)]").unwrap();
        let run_worker_source = &source[run_worker_index..tests_index];
        let renewal_loss_index = run_worker_source.find("renewal_lost_rx").unwrap();
        let process_index = run_worker_source.find("process_scan_task(").unwrap();

        assert!(run_worker_source.contains("tokio::select!"));
        assert!(renewal_loss_index < process_index);
        assert!(run_worker_source.contains("scan job lease ownership lost"));
    }

    #[test]
    fn scan_job_renewal_errors_report_ownership_loss() {
        let source = include_str!("lib.rs");
        let start = source.find("fn spawn_scan_job_lease_renewal(").unwrap();
        let end = source[start..]
            .find("async fn finish_scan_job_for_result")
            .unwrap()
            + start;
        let renewal_source = &source[start..end];

        assert!(renewal_source.contains("ownership_lost.send(true)"));
        assert!(renewal_source.contains("failed to renew scan job lease"));
    }

    #[test]
    fn scan_job_finalization_checks_ownership_before_success_audit() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let tests_index = source.find("#[cfg(test)]").unwrap();
        let run_worker_source = &source[run_worker_index..tests_index];
        let finish_job_index = run_worker_source.find("finish_scan_job_and_run_for_result(").unwrap();
        let finalizer_source = &source[source.find("async fn finish_scan_job_and_run_for_result").unwrap()..];
        let finish_run_index = finalizer_source.find("finish_scan_run_for_result_in_transaction").unwrap();

        assert!(run_worker_source[finish_job_index..].contains("job_finalized = false"));
        assert!(finalizer_source[..finish_run_index].contains("finish_scan_job_for_result_in_transaction"));
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
    fn worker_scan_entrypoints_do_not_call_single_active_provider_lookup() {
        let source = include_str!("lib.rs");
        let end = source.find("#[cfg(test)]").expect("test module");
        let source = &source[..end];

        assert!(!source.contains("active_rpc_provider_for_chain(pool, context.chain_id).await?"));
        assert!(
            source
                .matches("active_rpc_provider_candidates(pool, context.chain_id, now).await?")
                .count()
                >= 3
        );
    }

    #[test]
    fn evm_erc20_scan_chunks_logs_before_updating_cursor() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn scan_evm_erc20_transfers")
            .expect("erc20 scanner exists");
        let end = source[start..]
            .find("pub async fn scan_evm_address_with_provider")
            .expect("next function exists")
            + start;
        let scanner = &source[start..end];

        assert!(
            scanner.contains("bounded_block_ranges(from_block, to_block, EVM_LOG_MAX_BLOCK_SPAN)?")
        );
        assert!(scanner.contains("for range in ranges"));
        assert!(scanner.contains("range.from_block"));
        assert!(scanner.contains("range.to_block"));
        assert!(scanner.contains("last_successful_block = Some(range.to_block)"));
    }

    #[test]
    fn evm_scan_uses_provider_candidates_and_health_records() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn scan_evm_address(")
            .expect("EVM scan function");
        let end = source
            .find("pub async fn scan_tron_address(")
            .expect("TRON scan function");
        let source = &source[start..end];

        assert!(
            source.contains("active_rpc_provider_candidates(pool, context.chain_id, now).await?")
        );
        assert!(source.contains(
            "try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await?"
        ));
        assert!(source.contains("record_provider_success(pool, provider.id, now).await"));
        assert!(source.contains("record_provider_failure(pool, provider.id, now, &error).await"));
    }

    #[test]
    fn evm_scan_has_with_provider_function_for_single_candidate_attempt() {
        let source = include_str!("lib.rs");
        let end = source.find("#[cfg(test)]").expect("test module");
        let source = &source[..end];

        assert!(source.contains("scan_evm_address_with_provider"));
        assert!(source.contains("provider_capacity_error(context.chain_id)"));
    }

    #[test]
    fn evm_scan_reuses_candidate_context_for_provider_attempts() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn scan_evm_address_with_provider(")
            .expect("EVM provider attempt function");
        let end = source
            .find("pub async fn scan_evm_address(")
            .expect("EVM candidate loop function");
        let provider_attempt = &source[start..end];

        assert!(provider_attempt.contains("context: &ScanAddressContext"));
        assert!(provider_attempt.contains("ensure_provider_matches_context(provider, context)?"));
        assert!(!provider_attempt.contains("get_scan_address_context"));
    }

    #[test]
    fn evm_scan_uses_selected_assets_for_native_and_erc20_paths() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn scan_evm_address_with_provider(")
            .expect("EVM provider function");
        let end = source
            .find("pub async fn scan_evm_address(")
            .expect("EVM scan function");
        let evm = &source[start..end];

        assert!(evm.contains("selected_assets_for_address(pool, context.id).await?"));
        assert!(evm.contains("native_asset_selected(&selected_assets, &native_asset)"));
        assert!(evm.contains("&selected_assets"));
    }

    #[test]
    fn worker_no_longer_scans_all_active_assets_for_transfer_paths() {
        let source = include_str!("lib.rs");
        let end = source.find("#[cfg(test)]").expect("test module");
        let production = &source[..end];

        assert!(
            !production.contains("active_erc20_assets_for_chain(pool, context.chain_id).await?")
        );
        assert!(!production
            .contains("active_assets_for_chain_by_type(pool, context.chain_id, \"trc20\").await?"));
        assert!(production.contains("selected_assets_by_type(selected_assets, \"erc20\")"));
        assert!(production.contains("selected_assets_by_type(&selected_assets, \"trc20\")"));
    }

    #[test]
    fn btc_scan_skips_when_native_asset_is_not_selected() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn scan_btc_address_with_provider(")
            .expect("BTC provider function");
        let end = source
            .find("pub async fn scan_btc_address(")
            .expect("BTC scan function");
        let btc = &source[start..end];

        assert!(btc.contains("selected_assets_for_address(pool, context.id).await?"));
        assert!(btc.contains("if !native_asset_selected(&selected_assets, &native_asset)"));
        assert!(btc.contains("return Ok(Vec::new())"));
    }

    #[test]
    fn evm_scan_attempt_exhaustion_is_explicitly_tested() {
        let source = include_str!("lib.rs");
        let start = source
            .find("pub async fn scan_evm_address(")
            .expect("EVM candidate loop function");
        let end = source
            .find("pub async fn scan_tron_address(")
            .expect("TRON scan function");
        let evm_loop = &source[start..end];

        assert!(evm_loop.contains("provider_attempts_exhausted_error"));
    }

    #[test]
    fn tron_and_btc_scans_have_with_provider_functions() {
        let source = include_str!("lib.rs");
        let end = source.find("#[cfg(test)]").expect("test module");
        let source = &source[..end];

        assert!(source.contains("pub async fn scan_tron_address_with_provider("));
        assert!(source.contains("pub async fn scan_btc_address_with_provider("));
    }

    #[test]
    fn tron_and_btc_scan_entrypoints_record_provider_health() {
        let source = include_str!("lib.rs");
        let tron_start = source
            .find("pub async fn scan_tron_address(")
            .expect("TRON scan function");
        let tron_end = source
            .find("pub async fn scan_btc_address(")
            .expect("BTC scan function");
        let btc_start = tron_end;
        let btc_end = source
            .find("pub async fn process_scan_task(")
            .expect("process scan function");
        let scan_bodies = [&source[tron_start..tron_end], &source[btc_start..btc_end]];

        for scan_body in scan_bodies {
            assert!(scan_body
                .contains("active_rpc_provider_candidates(pool, context.chain_id, now).await?"));
            assert!(scan_body.contains(
                "try_acquire_provider_qps(redis, provider.id, provider.qps_limit, now).await?"
            ));
            assert!(scan_body.contains("record_provider_success(pool, provider.id, now).await"));
            assert!(
                scan_body.contains("record_provider_failure(pool, provider.id, now, &error).await")
            );
        }
    }

    #[test]
    fn tron_and_btc_scan_reuse_candidate_context_for_provider_attempts() {
        let source = include_str!("lib.rs");
        let end = source.find("#[cfg(test)]").expect("test module");
        let source = &source[..end];
        let tron_start = source
            .find("pub async fn scan_tron_address_with_provider(")
            .expect("TRON provider attempt function");
        let tron_end = source
            .find("pub async fn scan_tron_address(")
            .expect("TRON candidate loop function");
        let btc_start = source
            .find("pub async fn scan_btc_address_with_provider(")
            .expect("BTC provider attempt function");
        let btc_end = source
            .find("pub async fn scan_btc_address(")
            .expect("BTC candidate loop function");
        let provider_attempts = [&source[tron_start..tron_end], &source[btc_start..btc_end]];

        for provider_attempt in provider_attempts {
            assert!(provider_attempt.contains("context: &ScanAddressContext"));
            assert!(
                provider_attempt.contains("ensure_provider_matches_context(provider, context)?")
            );
            assert!(!provider_attempt.contains("get_scan_address_context"));
        }
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
