use coin_listener_core::{
    models::{AddressEventDraft, Asset, ScanAddressContext},
    AppError, AppResult,
};
use num_bigint::BigUint;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{str::FromStr, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TronBalance {
    pub balance_raw: String,
    pub balance_decimal: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TronPage {
    pub data: Vec<Value>,
    pub next_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTronTransfer {
    pub tx_hash: String,
    pub cursor_value: i64,
    pub block_number: Option<i64>,
    pub log_index: Option<i32>,
    pub from_address: String,
    pub to_address: String,
    pub amount_raw: String,
    pub amount_decimal: String,
    pub token_contract: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Trc20TransferPayload {
    transaction_id: String,
    block_number: Option<i64>,
    block_timestamp: Option<i64>,
    from: String,
    to: String,
    value: String,
    token_info: Trc20TokenInfo,
}

#[derive(Debug, Deserialize)]
struct Trc20TokenInfo {
    address: String,
}

#[derive(Debug, Deserialize)]
struct TrxTransferPayload {
    #[serde(rename = "txID")]
    tx_id: String,
    #[serde(rename = "blockNumber")]
    block_number: i64,
    #[serde(rename = "block_timestamp")]
    block_timestamp: i64,
    raw_data: TrxRawData,
}

#[derive(Debug, Deserialize)]
struct TrxRawData {
    contract: Vec<TrxContract>,
}

#[derive(Debug, Deserialize)]
struct TrxContract {
    #[serde(rename = "type")]
    contract_type: String,
    parameter: TrxContractParameter,
}

#[derive(Debug, Deserialize)]
struct TrxContractParameter {
    value: TrxContractValue,
}

#[derive(Debug, Deserialize)]
struct TrxContractValue {
    owner_address: Option<String>,
    to_address: Option<String>,
    amount: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrxTransferDecode {
    Transfer(DecodedTronTransfer),
    Skip,
}

pub fn normalize_tron_address(address: &str) -> AppResult<String> {
    let address = address.trim();
    if address.starts_with('T')
        && (26..=36).contains(&address.len())
        && address.chars().all(is_base58_like_character)
    {
        return Ok(address.to_string());
    }

    Err(AppError::Validation(format!(
        "invalid tron address {address}"
    )))
}

pub fn tron_transfer_direction(from: &str, to: &str, watched: &str) -> &'static str {
    let from = from.trim();
    let to = to.trim();
    let watched = watched.trim();

    if from == watched && to == watched {
        "self"
    } else if to == watched {
        "in"
    } else if from == watched {
        "out"
    } else {
        "unknown"
    }
}

pub fn decode_tron_balance(balance_sun: i64, decimals: i32) -> AppResult<TronBalance> {
    if balance_sun < 0 {
        return Err(AppError::Validation(format!(
            "invalid tron balance {balance_sun}: must be non-negative"
        )));
    }
    let balance_raw = balance_sun.to_string();
    let balance_decimal = raw_to_decimal_string(&balance_raw, decimals)?;
    Ok(TronBalance {
        balance_raw,
        balance_decimal,
    })
}

pub fn decode_trc20_transfer(
    payload: &Value,
    expected_contract: &str,
    decimals: i32,
) -> AppResult<DecodedTronTransfer> {
    decode_trc20_transfer_at_index(payload, expected_contract, decimals, 0)
}

pub fn decode_trc20_transfer_at_index(
    payload: &Value,
    expected_contract: &str,
    decimals: i32,
    fallback_log_index: i32,
) -> AppResult<DecodedTronTransfer> {
    let payload: Trc20TransferPayload =
        serde_json::from_value(payload.clone()).map_err(|error| {
            AppError::Validation(format!("invalid trc20 transfer payload: {error}"))
        })?;
    validate_tx_hash(&payload.transaction_id, "transaction_id")?;

    let expected_contract = normalize_tron_address(expected_contract)?;
    let token_contract = normalize_tron_address(&payload.token_info.address)?;
    if token_contract != expected_contract {
        return Err(AppError::Validation(format!(
            "invalid trc20 contract {token_contract}: expected {expected_contract}"
        )));
    }

    let block_number = match payload.block_number {
        Some(value) => Some(validate_non_negative_i64(value, "block_number")?),
        None => None,
    };
    let cursor_value = validate_non_negative_i64(
        payload.block_timestamp.ok_or_else(|| {
            AppError::Validation("invalid trc20 transfer: missing cursor".to_string())
        })?,
        "block_timestamp",
    )?;
    let amount_raw = parse_non_negative_decimal_string(&payload.value, "value")?;
    let amount_decimal = raw_to_decimal_string(&amount_raw, decimals)?;
    let log_index = tron_transfer_log_index(
        payload.transaction_id.as_str(),
        &payload.from,
        &payload.to,
        &amount_raw,
        Some(token_contract.as_str()),
        fallback_log_index,
    );

    Ok(DecodedTronTransfer {
        tx_hash: payload.transaction_id,
        cursor_value,
        block_number,
        log_index: Some(log_index),
        from_address: normalize_tron_address(&payload.from)?,
        to_address: normalize_tron_address(&payload.to)?,
        amount_raw,
        amount_decimal,
        token_contract: Some(token_contract),
    })
}

pub fn decode_trx_transfer(payload: &Value, decimals: i32) -> AppResult<DecodedTronTransfer> {
    match try_decode_trx_transfer_at_index(payload, decimals, 0)? {
        TrxTransferDecode::Transfer(transfer) => Ok(transfer),
        TrxTransferDecode::Skip => Err(AppError::Validation(
            "invalid trx transfer: missing TransferContract".to_string(),
        )),
    }
}

pub fn decode_trx_transfer_at_index(
    payload: &Value,
    decimals: i32,
    fallback_log_index: i32,
) -> AppResult<DecodedTronTransfer> {
    match try_decode_trx_transfer_at_index(payload, decimals, fallback_log_index)? {
        TrxTransferDecode::Transfer(transfer) => Ok(transfer),
        TrxTransferDecode::Skip => Err(AppError::Validation(
            "invalid trx transfer: missing TransferContract".to_string(),
        )),
    }
}

pub fn try_decode_trx_transfer_at_index(
    payload: &Value,
    decimals: i32,
    fallback_log_index: i32,
) -> AppResult<TrxTransferDecode> {
    let payload: TrxTransferPayload = serde_json::from_value(payload.clone())
        .map_err(|error| AppError::Validation(format!("invalid trx transfer payload: {error}")))?;
    validate_tx_hash(&payload.tx_id, "txID")?;
    let block_number = validate_non_negative_i64(payload.block_number, "blockNumber")?;
    let cursor_value = validate_non_negative_i64(payload.block_timestamp, "block_timestamp")?;
    let Some(transfer) = payload
        .raw_data
        .contract
        .into_iter()
        .find(|contract| contract.contract_type == "TransferContract")
    else {
        return Ok(TrxTransferDecode::Skip);
    };
    let owner_address = transfer.parameter.value.owner_address.ok_or_else(|| {
        AppError::Validation("invalid trx transfer: missing owner_address".to_string())
    })?;
    let owner_address = normalize_tron_address(&owner_address)?;
    let to_address = transfer.parameter.value.to_address.ok_or_else(|| {
        AppError::Validation("invalid trx transfer: missing to_address".to_string())
    })?;
    let to_address = normalize_tron_address(&to_address)?;
    let amount =
        transfer.parameter.value.amount.ok_or_else(|| {
            AppError::Validation("invalid trx transfer: missing amount".to_string())
        })?;
    if amount < 0 {
        return Err(AppError::Validation(format!(
            "invalid trx transfer amount {amount}: must be non-negative"
        )));
    }
    let amount_raw = amount.to_string();
    let amount_decimal = raw_to_decimal_string(&amount_raw, decimals)?;
    let log_index = tron_transfer_log_index(
        payload.tx_id.as_str(),
        &owner_address,
        &to_address,
        &amount_raw,
        None,
        fallback_log_index,
    );

    Ok(TrxTransferDecode::Transfer(DecodedTronTransfer {
        tx_hash: payload.tx_id,
        cursor_value,
        block_number: Some(block_number),
        log_index: Some(log_index),
        from_address: owner_address,
        to_address,
        amount_raw,
        amount_decimal,
        token_contract: None,
    }))
}

#[derive(Debug, Clone)]
pub struct TronClient {
    base_url: String,
    client: reqwest::Client,
}

pub fn account_transactions_query(
    min_timestamp: i64,
    fingerprint: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("only_confirmed", "true".to_string()),
        ("limit", "200".to_string()),
        ("min_timestamp", min_timestamp.to_string()),
    ];
    push_fingerprint(&mut query, fingerprint);
    query
}

pub fn account_trc20_transfers_query(
    contract_address: String,
    min_timestamp: i64,
    fingerprint: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("only_confirmed", "true".to_string()),
        ("limit", "200".to_string()),
        ("contract_address", contract_address),
        ("min_timestamp", min_timestamp.to_string()),
    ];
    push_fingerprint(&mut query, fingerprint);
    query
}

fn push_fingerprint(query: &mut Vec<(&'static str, String)>, fingerprint: Option<&str>) {
    if let Some(fingerprint) = fingerprint
        .map(str::trim)
        .filter(|fingerprint| !fingerprint.is_empty())
    {
        query.push(("fingerprint", fingerprint.to_string()));
    }
}

impl TronClient {
    pub fn new(base_url: String, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("valid TRON client");
        Self { base_url, client }
    }

    pub fn account_transactions_path(&self, address: &str) -> AppResult<String> {
        let address = normalize_tron_address(address)?;
        Ok(format!("/v1/accounts/{address}/transactions"))
    }

    pub fn account_trc20_path(&self, address: &str) -> AppResult<String> {
        let address = normalize_tron_address(address)?;
        Ok(format!("/v1/accounts/{address}/transactions/trc20"))
    }

    pub async fn account_transactions(
        &self,
        address: &str,
        min_timestamp: i64,
    ) -> AppResult<Vec<Value>> {
        self.account_transactions_page(address, min_timestamp, None)
            .await
            .map(|page| page.data)
    }

    pub async fn account_transactions_page(
        &self,
        address: &str,
        min_timestamp: i64,
        fingerprint: Option<&str>,
    ) -> AppResult<TronPage> {
        let path = self.account_transactions_path(address)?;
        let query = account_transactions_query(min_timestamp, fingerprint);
        let body = self
            .get_json_body("account transactions", &path, &query)
            .await?;
        parse_tron_page(body, "account transactions")
    }

    pub async fn account_trc20_transfers(
        &self,
        address: &str,
        contract_address: &str,
        min_timestamp: i64,
    ) -> AppResult<Vec<Value>> {
        self.account_trc20_transfers_page(address, contract_address, min_timestamp, None)
            .await
            .map(|page| page.data)
    }

    pub async fn account_trc20_transfers_page(
        &self,
        address: &str,
        contract_address: &str,
        min_timestamp: i64,
        fingerprint: Option<&str>,
    ) -> AppResult<TronPage> {
        let path = self.account_trc20_path(address)?;
        let contract_address = normalize_tron_address(contract_address)?;
        let query = account_trc20_transfers_query(contract_address, min_timestamp, fingerprint);
        let body = self.get_json_body("TRC20 transfers", &path, &query).await?;
        parse_tron_page(body, "TRC20 transfers")
    }

    async fn get_json_body(
        &self,
        operation: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> AppResult<Value> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let response = self
            .client
            .get(&url)
            .query(query)
            .send()
            .await
            .map_err(|error| {
                AppError::Config(format_tron_request_error(
                    operation,
                    &self.base_url,
                    &error.without_url().to_string(),
                ))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            AppError::Config(format!(
                "TRON {operation} response body failed: {}",
                error.without_url()
            ))
        })?;
        if !status.is_success() {
            return Err(AppError::Config(format_tron_status_error(
                operation,
                &self.base_url,
                status,
                &body,
            )));
        }
        serde_json::from_str(&body).map_err(|error| {
            AppError::Validation(format!("invalid TRON {operation} response json: {error}"))
        })
    }
}

pub fn format_tron_request_error(operation: &str, base_url: &str, error: &str) -> String {
    format!(
        "TRON {operation} request failed: {}",
        redact_provider_url(error, base_url)
    )
}

pub fn format_tron_status_error(
    operation: &str,
    base_url: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> String {
    format!(
        "TRON {operation} returned http {status}: {}",
        redact_provider_url(body, base_url)
    )
}

fn redact_provider_url(error: &str, base_url: &str) -> String {
    let base_url = base_url.trim();
    if base_url.is_empty() {
        return error.to_string();
    }

    let without_trailing_slash = base_url.trim_end_matches('/');
    let with_trailing_slash = format!("{without_trailing_slash}/");
    let mut candidates = vec![
        base_url,
        with_trailing_slash.as_str(),
        without_trailing_slash,
    ];
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.len()));
    candidates.dedup();

    let mut redacted = error.to_string();
    for candidate in candidates {
        if !candidate.is_empty() {
            redacted = redacted.replace(candidate, "[redacted provider url]");
        }
    }
    redacted
}

pub fn parse_data_array(body: Value, operation: &str) -> AppResult<Vec<Value>> {
    parse_tron_page(body, operation).map(|page| page.data)
}

pub fn parse_tron_page(body: Value, operation: &str) -> AppResult<TronPage> {
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| {
            AppError::Validation(format!("TRON {operation} response missing data array"))
        })?;
    let next_fingerprint = body
        .get("meta")
        .and_then(|meta| meta.get("fingerprint"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|fingerprint| !fingerprint.is_empty())
        .map(ToString::to_string);

    Ok(TronPage {
        data,
        next_fingerprint,
    })
}

pub fn tron_transfer_event_draft(
    context: &ScanAddressContext,
    asset: &Asset,
    transfer: DecodedTronTransfer,
) -> AddressEventDraft {
    let direction = tron_transfer_direction(
        &transfer.from_address,
        &transfer.to_address,
        &context.address,
    );

    AddressEventDraft {
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: asset.id,
        event_type: "transfer".to_string(),
        direction: direction.to_string(),
        is_transfer: true,
        tx_hash: Some(transfer.tx_hash),
        log_index: transfer.log_index,
        block_number: transfer.block_number,
        block_hash: None,
        confirmations: 0,
        from_address: Some(transfer.from_address),
        to_address: Some(transfer.to_address),
        amount_raw: Some(transfer.amount_raw),
        amount_decimal: Some(transfer.amount_decimal),
        balance_before_raw: None,
        balance_after_raw: None,
        balance_delta_raw: None,
        metadata: json!({
            "source": "tron_transfer",
            "cursor_value": transfer.cursor_value,
            "token_contract": transfer.token_contract,
        }),
    }
}

fn is_base58_like_character(character: char) -> bool {
    character.is_ascii_alphanumeric() && !matches!(character, '0' | 'O' | 'I' | 'l')
}

fn validate_non_negative_i64(value: i64, field: &str) -> AppResult<i64> {
    if value >= 0 {
        Ok(value)
    } else {
        Err(AppError::Validation(format!(
            "invalid tron {field} {value}: must be non-negative"
        )))
    }
}

fn tron_transfer_log_index(
    tx_hash: &str,
    from_address: &str,
    to_address: &str,
    amount_raw: &str,
    token_contract: Option<&str>,
    fallback_log_index: i32,
) -> i32 {
    let mut hasher = Sha256::new();
    hasher.update(tx_hash.as_bytes());
    hasher.update(from_address.as_bytes());
    hasher.update(to_address.as_bytes());
    hasher.update(amount_raw.as_bytes());
    if let Some(token_contract) = token_contract {
        hasher.update(token_contract.as_bytes());
    }
    hasher.update(fallback_log_index.to_be_bytes());
    let digest = hasher.finalize();
    i32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) & i32::MAX
}

fn raw_to_decimal_string(raw: &str, decimals: i32) -> AppResult<String> {
    if decimals < 0 {
        return Err(AppError::Validation(
            "asset decimals cannot be negative".to_string(),
        ));
    }
    let _ = BigUint::from_str(raw)
        .map_err(|error| AppError::Validation(format!("invalid decimal amount {raw}: {error}")))?;
    let decimals = decimals as usize;
    if decimals == 0 {
        return Ok(raw.to_string());
    }
    let padded = if raw.len() <= decimals {
        format!("{}{}", "0".repeat(decimals + 1 - raw.len()), raw)
    } else {
        raw.to_string()
    };
    let split_at = padded.len() - decimals;
    let integer = &padded[..split_at];
    let fraction = padded[split_at..].trim_end_matches('0');
    if fraction.is_empty() {
        Ok(format!("{integer}.0"))
    } else {
        Ok(format!("{integer}.{fraction}"))
    }
}

fn parse_non_negative_decimal_string(value: &str, field: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') {
        return Err(AppError::Validation(format!(
            "invalid trc20 {field} {value}"
        )));
    }
    let parsed = BigUint::from_str(value)
        .map_err(|error| AppError::Validation(format!("invalid trc20 {field} {value}: {error}")))?;
    Ok(parsed.to_string())
}

fn validate_tx_hash(value: &str, field: &str) -> AppResult<()> {
    if value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "invalid tron {field} {value}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decode_trc20_transfer, decode_tron_balance, decode_trx_transfer, normalize_tron_address,
        tron_transfer_direction, try_decode_trx_transfer_at_index, DecodedTronTransfer,
        TrxTransferDecode,
    };
    use coin_listener_core::{models::ScanAddressContext, AppError};
    use serde_json::{json, Value};
    use uuid::Uuid;

    const FROM_ADDRESS: &str = "TLa2f6VPqDgRE67v1736s7bJ8Ray5wYjU7";
    const TO_ADDRESS: &str = "TMuA6YqfCeX8EhbfYEg5y7S4DqzSJireY9";
    const TOKEN_CONTRACT: &str = "TXLAQ63Xg1NAzckPwKHvzw7CSEmLMEqcdj";

    #[test]
    fn tron_client_builds_account_paths_without_double_slashes() {
        let client = super::TronClient::new(
            "https://api.trongrid.io/".to_string(),
            std::time::Duration::from_secs(5),
        );

        assert_eq!(
            client
                .account_transactions_path("TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh")
                .unwrap(),
            "/v1/accounts/TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh/transactions"
        );
        assert_eq!(
            client
                .account_trc20_path("TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh")
                .unwrap(),
            "/v1/accounts/TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh/transactions/trc20"
        );
    }

    #[test]
    fn tron_parse_page_reads_data_and_fingerprint() {
        let page = super::parse_tron_page(
            json!({
                "data": [{ "transaction_id": tron_hash(1) }],
                "meta": { "fingerprint": "fingerprint-1" }
            }),
            "account transactions",
        )
        .unwrap();

        assert_eq!(page.data.len(), 1);
        assert_eq!(page.next_fingerprint, Some("fingerprint-1".to_string()));
    }

    #[test]
    fn tron_parse_page_treats_missing_or_empty_fingerprint_as_last_page() {
        let missing =
            super::parse_tron_page(json!({ "data": [] }), "account transactions").unwrap();
        let empty = super::parse_tron_page(
            json!({ "data": [], "meta": { "fingerprint": "  " } }),
            "account transactions",
        )
        .unwrap();

        assert_eq!(missing.next_fingerprint, None);
        assert_eq!(empty.next_fingerprint, None);
    }

    #[test]
    fn tron_account_queries_append_fingerprint_when_present() {
        let without_fingerprint = super::account_transactions_query(1_710_000_000_000, None);
        let with_fingerprint =
            super::account_transactions_query(1_710_000_000_000, Some(" fingerprint-2 "));

        assert!(!without_fingerprint
            .iter()
            .any(|(key, _)| *key == "fingerprint"));
        assert!(with_fingerprint.contains(&("fingerprint", "fingerprint-2".to_string())));
    }

    #[test]
    fn tron_trc20_queries_include_contract_and_fingerprint() {
        let query = super::account_trc20_transfers_query(
            TOKEN_CONTRACT.to_string(),
            1_710_000_000_000,
            Some("fingerprint-3"),
        );

        assert!(query.contains(&("contract_address", TOKEN_CONTRACT.to_string())));
        assert!(query.contains(&("fingerprint", "fingerprint-3".to_string())));
    }

    #[test]
    fn tron_status_errors_do_not_include_provider_url() {
        let message = super::format_tron_status_error(
            "TRC20 transfers",
            "https://api.trongrid.io/provider-key/",
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "upstream https://api.trongrid.io/provider-key/ failed",
        );

        assert!(message.contains("TRC20 transfers"));
        assert!(message.contains("429 Too Many Requests"));
        assert!(message.contains("[redacted provider url]"));
        assert!(!message.contains("trongrid.io"));
        assert!(!message.contains("provider-key"));
    }

    #[test]
    fn tron_request_errors_do_not_include_provider_url() {
        let message = super::format_tron_request_error(
            "account transactions",
            "https://api.trongrid.io/private-key/",
            "request to https://api.trongrid.io/private-key failed: connection refused",
        );

        assert!(message.contains("account transactions"));
        assert!(message.contains("connection refused"));
        assert!(!message.contains("trongrid.io"));
        assert!(!message.contains("private-key"));
    }

    #[test]
    fn tron_transfer_event_draft_maps_transfer_fields() {
        let context = scan_context("TQ6p7JAFM2Z2V5Q3U6QwY7Xx9z5xZQkP8E");
        let asset = asset("trc20", "USDT", Some("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"));
        let transfer = DecodedTronTransfer {
            tx_hash: tron_hash(3),
            cursor_value: 65_000_000,
            block_number: Some(65_000_000),
            log_index: Some(7),
            from_address: "TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh".to_string(),
            to_address: context.address.clone(),
            amount_raw: "2500000".to_string(),
            amount_decimal: "2.5".to_string(),
            token_contract: Some("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t".to_string()),
        };

        let draft = super::tron_transfer_event_draft(&context, &asset, transfer);

        assert_eq!(draft.tenant_id, context.tenant_id);
        assert_eq!(draft.chain_id, context.chain_id);
        assert_eq!(draft.address_id, context.id);
        assert_eq!(draft.asset_id, asset.id);
        assert_eq!(draft.event_type, "transfer");
        assert!(draft.is_transfer);
        assert_eq!(draft.direction, "in");
        assert_eq!(draft.block_number, Some(65_000_000));
        assert_eq!(draft.tx_hash, Some(tron_hash(3)));
        assert_eq!(draft.log_index, Some(7));
        assert_eq!(
            draft.from_address,
            Some("TJmmqjb1DK9TTZbQXzRQ2AuA94z4gKAPFh".to_string())
        );
        assert_eq!(draft.to_address, Some(context.address));
        assert_eq!(draft.amount_raw, Some("2500000".to_string()));
        assert_eq!(draft.amount_decimal, Some("2.5".to_string()));
        assert_eq!(draft.metadata["source"], "tron_transfer");
        assert_eq!(draft.metadata["cursor_value"], 65_000_000);
        assert_eq!(
            draft.metadata["token_contract"],
            "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"
        );
    }

    #[test]
    fn tron_address_normalization_accepts_base58_like_and_rejects_evm_hex() {
        assert_eq!(
            normalize_tron_address("  TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE  ").unwrap(),
            "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE"
        );

        assert!(matches!(
            normalize_tron_address("0x1111111111111111111111111111111111111111"),
            Err(AppError::Validation(message)) if message.contains("address")
        ));
        assert!(matches!(
            normalize_tron_address("Tshort"),
            Err(AppError::Validation(message)) if message.contains("address")
        ));
        assert!(matches!(
            normalize_tron_address("TLa2f6VPqDgRE67v1736s7bJ8Ray5wYjU-"),
            Err(AppError::Validation(message)) if message.contains("address")
        ));
        assert!(matches!(
            normalize_tron_address("TLa2f6VPqDgRE67v1736s7bJ8Ray5wYj0"),
            Err(AppError::Validation(message)) if message.contains("address")
        ));
    }

    #[test]
    fn tron_trc20_payload_decodes_transfer_fields_and_decimal_amount() {
        let payload = trc20_payload(json!({
            "block_number": 54_321,
            "block_timestamp": 1_710_000_000_000i64,
            "value": "2500000"
        }));

        let decoded = decode_trc20_transfer(&payload, TOKEN_CONTRACT, 6).unwrap();

        assert_eq!(decoded.tx_hash, tron_hash(1));
        assert_eq!(decoded.cursor_value, 1_710_000_000_000i64);
        assert_eq!(decoded.block_number, Some(54_321));
        assert!(decoded.log_index.is_some());
        assert_eq!(decoded.from_address, FROM_ADDRESS);
        assert_eq!(decoded.to_address, TO_ADDRESS);
        assert_eq!(decoded.amount_raw, "2500000");
        assert_eq!(decoded.amount_decimal, "2.5");
        assert_eq!(decoded.token_contract, Some(TOKEN_CONTRACT.to_string()));
    }

    #[test]
    fn tron_trc20_payload_uses_timestamp_cursor_when_block_number_is_absent() {
        let payload = trc20_payload(json!({
            "block_timestamp": 1_710_000_000_000i64,
            "value": "1000000"
        }));

        let decoded = decode_trc20_transfer(&payload, TOKEN_CONTRACT, 6).unwrap();

        assert_eq!(decoded.cursor_value, 1_710_000_000_000i64);
        assert_eq!(decoded.block_number, None);
        assert_eq!(decoded.amount_raw, "1000000");
        assert_eq!(decoded.amount_decimal, "1.0");
    }

    #[test]
    fn tron_trc20_payload_rejects_wrong_contract_with_validation_error() {
        let payload = trc20_payload(json!({
            "block_number": 54_321,
            "block_timestamp": 1_710_000_000_000i64,
            "value": "2500000"
        }));

        let result = decode_trc20_transfer(&payload, "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE", 6);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message.contains("contract")
        ));
    }

    #[test]
    fn tron_trc20_payload_rejects_negative_block_number() {
        let payload = trc20_payload(json!({
            "block_number": -1,
            "block_timestamp": 1_710_000_000_000i64,
            "value": "2500000"
        }));

        let result = decode_trc20_transfer(&payload, TOKEN_CONTRACT, 6);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message.contains("block_number")
        ));
    }

    #[test]
    fn tron_trc20_payload_rejects_negative_timestamp_cursor() {
        let payload = trc20_payload(json!({
            "block_timestamp": -1,
            "value": "2500000"
        }));

        let result = decode_trc20_transfer(&payload, TOKEN_CONTRACT, 6);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message.contains("block_timestamp")
        ));
    }

    #[test]
    fn tron_trx_payload_decodes_transfer_contract_fields() {
        let payload = trx_payload(60_000);

        let decoded = decode_trx_transfer(&payload, 6).unwrap();

        assert_eq!(decoded.tx_hash, tron_hash(2));
        assert_eq!(decoded.cursor_value, 1_710_000_000_000i64);
        assert_eq!(decoded.block_number, Some(60_000));
        assert!(decoded.log_index.is_some());
        assert_eq!(decoded.from_address, FROM_ADDRESS);
        assert_eq!(decoded.to_address, TO_ADDRESS);
        assert_eq!(decoded.amount_raw, "1234567");
        assert_eq!(decoded.amount_decimal, "1.234567");
        assert_eq!(decoded.token_contract, None);
    }

    #[test]
    fn tron_trx_payload_rejects_negative_block_number() {
        let payload = trx_payload(-1);

        let result = decode_trx_transfer(&payload, 6);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message.contains("blockNumber")
        ));
    }

    #[test]
    fn tron_non_transfer_contract_is_skipped_by_try_decode() {
        let payload = json!({
            "txID": tron_hash(4),
            "blockNumber": 60_001,
            "block_timestamp": 1_710_000_000_001i64,
            "raw_data": {
                "contract": [
                    {
                        "type": "TriggerSmartContract",
                        "parameter": { "value": {} }
                    }
                ]
            }
        });

        let result = try_decode_trx_transfer_at_index(&payload, 6, 0).unwrap();

        assert_eq!(result, TrxTransferDecode::Skip);
    }

    #[test]
    fn tron_balance_converts_sun_to_raw_and_decimal() {
        let one = decode_tron_balance(1_000_000, 6).unwrap();
        assert_eq!(one.balance_raw, "1000000");
        assert_eq!(one.balance_decimal, "1.0");

        let fractional = decode_tron_balance(1_234_567, 6).unwrap();
        assert_eq!(fractional.balance_raw, "1234567");
        assert_eq!(fractional.balance_decimal, "1.234567");
    }

    #[test]
    fn tron_transfer_direction_classifies_from_watched_perspective() {
        assert_eq!(
            tron_transfer_direction(FROM_ADDRESS, TO_ADDRESS, TO_ADDRESS),
            "in"
        );
        assert_eq!(
            tron_transfer_direction(FROM_ADDRESS, TO_ADDRESS, FROM_ADDRESS),
            "out"
        );
        assert_eq!(
            tron_transfer_direction(FROM_ADDRESS, FROM_ADDRESS, FROM_ADDRESS),
            "self"
        );
        assert_eq!(
            tron_transfer_direction(FROM_ADDRESS, TO_ADDRESS, TOKEN_CONTRACT),
            "unknown"
        );
    }

    fn scan_context(address: &str) -> ScanAddressContext {
        ScanAddressContext {
            id: Uuid::from_u128(101),
            tenant_id: Uuid::from_u128(102),
            chain_id: Uuid::from_u128(103),
            address: address.to_string(),
            scan_interval_seconds: 300,
            chain_type: "tron".to_string(),
        }
    }

    fn asset(
        asset_type: &str,
        symbol: &str,
        contract: Option<&str>,
    ) -> coin_listener_core::models::Asset {
        coin_listener_core::models::Asset {
            id: Uuid::from_u128(201),
            chain_id: Uuid::from_u128(103),
            asset_type: asset_type.to_string(),
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            contract_address: contract.map(ToOwned::to_owned),
            decimals: 6,
            is_builtin: true,
            status: "active".to_string(),
        }
    }

    fn trc20_payload(extra: Value) -> Value {
        let mut payload = json!({
            "transaction_id": tron_hash(1),
            "from": FROM_ADDRESS,
            "to": TO_ADDRESS,
            "token_info": {
                "address": TOKEN_CONTRACT
            }
        });
        let payload_object = payload.as_object_mut().unwrap();
        for (key, value) in extra.as_object().unwrap() {
            payload_object.insert(key.clone(), value.clone());
        }
        payload
    }

    fn trx_payload(block_number: i64) -> Value {
        json!({
            "txID": tron_hash(2),
            "blockNumber": block_number,
            "block_timestamp": 1_710_000_000_000i64,
            "raw_data": {
                "contract": [
                    {
                        "type": "TriggerSmartContract",
                        "parameter": { "value": {} }
                    },
                    {
                        "type": "TransferContract",
                        "parameter": {
                            "value": {
                                "owner_address": FROM_ADDRESS,
                                "to_address": TO_ADDRESS,
                                "amount": 1_234_567
                            }
                        }
                    }
                ]
            }
        })
    }

    fn tron_hash(value: u8) -> String {
        format!("{value:064x}")
    }
}
