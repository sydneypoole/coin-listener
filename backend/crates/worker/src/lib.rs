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
    scan_queue::ScanQueue,
};
use redis::aio::MultiplexedConnection;
use sqlx::PgPool;
use tracing::{info, warn};

pub const EVM_ERC20_TRANSFER_CURSOR: &str = "evm_erc20_transfer";
pub const EVM_TRANSFER_INITIAL_WINDOW_BLOCKS: i64 = 1_000;
pub const EVM_LOG_MAX_BLOCK_SPAN: i64 = 10_000;
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
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
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
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
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
    task: &ScanAddressTask,
    now: DateTime<Utc>,
) -> AppResult<Vec<AddressEvent>> {
    let context = repositories::get_scan_address_context(pool, task.address_id).await?;
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

    let rows = address_imports::pending_import_rows(
        pool,
        task.tenant_id,
        task.id,
        ADDRESS_IMPORT_ROW_BATCH_SIZE,
    )
    .await?;

    for row in rows {
        let request = CreateWatchedAddressRequest {
            tenant_id: Some(task.tenant_id),
            chain_id: task.chain_id,
            address: row.address,
            label: row.label,
            priority: row.priority.unwrap_or_else(|| task.priority.clone()),
            scan_interval_seconds: row
                .scan_interval_seconds
                .unwrap_or(task.scan_interval_seconds),
            transfer_filter_enabled: row
                .transfer_filter_enabled
                .unwrap_or(task.transfer_filter_enabled),
            balance_change_filter_enabled: row
                .balance_change_filter_enabled
                .unwrap_or(task.balance_change_filter_enabled),
            status: row.status.unwrap_or_else(|| task.address_status.clone()),
            asset_ids: task.asset_ids.clone(),
        };

        match repositories::create_watched_address(pool, request).await {
            Ok(address) => {
                address_imports::mark_import_row_success(
                    pool,
                    task.tenant_id,
                    task.id,
                    row.row_number,
                    address.id,
                )
                .await?;
            }
            Err(error) => {
                address_imports::mark_import_row_failed(
                    pool,
                    task.tenant_id,
                    task.id,
                    row.row_number,
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
        if let Err(error) = process_one_address_import_task(&pool, "worker", Utc::now()).await {
            warn!(error = %error, "address import task processing failed");
        }

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

        use crate::{
            bounded_block_ranges, evm_transfer_scan_range, BlockRange, EVM_ERC20_TRANSFER_CURSOR,
            EVM_LOG_MAX_BLOCK_SPAN,
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
            let ranges = bounded_block_ranges(5, 10_004, EVM_LOG_MAX_BLOCK_SPAN).unwrap();

            assert_eq!(
                ranges,
                vec![BlockRange {
                    from_block: 5,
                    to_block: 10_004,
                }]
            );
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

    #[test]
    fn address_import_batch_size_is_bounded() {
        assert_eq!(crate::ADDRESS_IMPORT_ROW_BATCH_SIZE, 50);
    }

    #[test]
    fn worker_source_processes_address_import_before_scan_dequeue() {
        let source = include_str!("lib.rs");
        let run_worker_index = source.find("pub async fn run_worker(").unwrap();
        let run_worker_source = &source[run_worker_index..];
        let import_index = run_worker_source
            .find("process_one_address_import_task(&pool")
            .unwrap();
        let dequeue_index = run_worker_source.find("scan_queue.dequeue").unwrap();

        assert!(import_index < dequeue_index);
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

        assert!(scanner.contains("bounded_block_ranges(from_block, to_block, EVM_LOG_MAX_BLOCK_SPAN)?"));
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
