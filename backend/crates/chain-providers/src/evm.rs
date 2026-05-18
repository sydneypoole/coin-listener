use coin_listener_core::{
    models::{
        AddressEventDraft, Asset, BalanceSnapshot, Provider, ScanAddressContext, WatchedAddress,
    },
    AppError, AppResult,
};
use num_bigint::{BigInt, BigUint};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{str::FromStr, time::Duration};

#[derive(Debug, Clone)]
pub struct EvmRpcClient {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvmBlockTag {
    Latest,
}

impl EvmBlockTag {
    pub fn as_param(self) -> &'static str {
        match self {
            Self::Latest => "latest",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmBalance {
    pub block_number: i64,
    pub balance_raw: String,
    pub balance_decimal: String,
}

pub const TRANSFER_TOPIC0: &str =
    "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmLogFilter {
    pub address: String,
    pub from_block: i64,
    pub to_block: i64,
    pub topics: Vec<Option<String>>,
}

impl EvmLogFilter {
    pub fn to_rpc_params(&self) -> AppResult<Value> {
        if self.from_block < 0 {
            return Err(AppError::Validation(format!(
                "invalid eth_getLogs fromBlock {}: must be non-negative",
                self.from_block
            )));
        }
        if self.to_block < 0 {
            return Err(AppError::Validation(format!(
                "invalid eth_getLogs toBlock {}: must be non-negative",
                self.to_block
            )));
        }
        if self.from_block > self.to_block {
            return Err(AppError::Validation(format!(
                "invalid eth_getLogs range: fromBlock {} exceeds toBlock {}",
                self.from_block, self.to_block
            )));
        }

        Ok(json!([{
            "address": self.address,
            "fromBlock": format!("0x{:x}", self.from_block),
            "toBlock": format!("0x{:x}", self.to_block),
            "topics": self.topics,
        }]))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EvmLog {
    pub address: String,
    pub topics: Vec<String>,
    pub data: String,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
    #[serde(rename = "logIndex")]
    pub log_index: Option<String>,
    #[serde(rename = "blockNumber")]
    pub block_number: Option<String>,
    #[serde(rename = "blockHash")]
    pub block_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedErc20Transfer {
    pub tx_hash: String,
    pub log_index: i32,
    pub block_number: i64,
    pub block_hash: Option<String>,
    pub from_address: String,
    pub to_address: String,
    pub amount_raw: String,
    pub amount_decimal: String,
    pub token_contract: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

impl EvmRpcClient {
    pub fn new(base_url: String, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("valid evm rpc client");
        Self { base_url, client }
    }

    pub async fn eth_block_number(&self) -> AppResult<i64> {
        let value = self.rpc_hex_result("eth_blockNumber", json!([])).await?;
        parse_hex_quantity_to_i64(&value)
    }

    pub async fn eth_get_balance(&self, address: &str, block: EvmBlockTag) -> AppResult<String> {
        self.rpc_hex_result("eth_getBalance", json!([address, block.as_param()]))
            .await
    }

    pub async fn eth_get_logs(&self, filter: EvmLogFilter) -> AppResult<Vec<EvmLog>> {
        let body = self
            .rpc_result_body("eth_getLogs", filter.to_rpc_params()?)
            .await?;
        parse_json_rpc_logs_result(&body, "eth_getLogs")
    }

    async fn rpc_result_body(&self, method: &str, params: Value) -> AppResult<String> {
        let response = self
            .client
            .post(&self.base_url)
            .json(&build_json_rpc_request(method, params))
            .send()
            .await
            .map_err(|error| {
                AppError::Config(format_rpc_request_error(
                    method,
                    &self.base_url,
                    &error.without_url().to_string(),
                ))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            AppError::Config(format_rpc_body_error(
                method,
                &error.without_url().to_string(),
            ))
        })?;
        if !status.is_success() {
            return Err(AppError::Config(format_rpc_status_error(
                method, status, &body,
            )));
        }
        Ok(body)
    }

    async fn rpc_hex_result(&self, method: &str, params: Value) -> AppResult<String> {
        let body = self.rpc_result_body(method, params).await?;
        parse_json_rpc_hex_result(&body, method)
    }
}

pub fn build_json_rpc_request(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
}

fn format_rpc_request_error(method: &str, _base_url: &str, error: &str) -> String {
    format!("evm rpc {method} request failed: {error}")
}

fn format_rpc_body_error(method: &str, error: &str) -> String {
    format!("evm rpc {method} response body failed: {error}")
}

fn format_rpc_status_error(method: &str, status: reqwest::StatusCode, body: &str) -> String {
    format!("evm rpc {method} returned http {status}: {body}")
}

pub fn parse_json_rpc_hex_result(payload: &str, method: &str) -> AppResult<String> {
    let response: JsonRpcResponse = serde_json::from_str(payload).map_err(|error| {
        AppError::Validation(format!("invalid evm rpc {method} response json: {error}"))
    })?;
    if let Some(error) = response.error {
        return Err(AppError::Validation(format!(
            "evm rpc {method} error {}: {}",
            error.code, error.message
        )));
    }
    let result = response
        .result
        .ok_or_else(|| AppError::Validation(format!("evm rpc {method} response missing result")))?;
    result
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Validation(format!("evm rpc {method} result is not a hex string")))
}

pub fn parse_json_rpc_logs_result(payload: &str, method: &str) -> AppResult<Vec<EvmLog>> {
    let response: JsonRpcResponse = serde_json::from_str(payload).map_err(|error| {
        AppError::Validation(format!("invalid evm rpc {method} response json: {error}"))
    })?;
    if let Some(error) = response.error {
        return Err(AppError::Validation(format!(
            "evm rpc {method} error {}: {}",
            error.code, error.message
        )));
    }
    let result = response
        .result
        .ok_or_else(|| AppError::Validation(format!("evm rpc {method} response missing result")))?;
    serde_json::from_value(result).map_err(|error| {
        AppError::Validation(format!("invalid evm rpc {method} logs result: {error}"))
    })
}

pub fn parse_hex_quantity_to_i64(hex: &str) -> AppResult<i64> {
    let digits = hex_digits(hex)?;
    i64::from_str_radix(digits, 16)
        .map_err(|error| AppError::Validation(format!("invalid hex quantity {hex}: {error}")))
}

pub fn parse_hex_u256_to_decimal_string(hex: &str) -> AppResult<String> {
    let digits = hex_digits(hex)?;
    BigUint::parse_bytes(digits.as_bytes(), 16)
        .map(|value| value.to_string())
        .ok_or_else(|| AppError::Validation(format!("invalid hex quantity {hex}")))
}

fn parse_abi_u256_to_decimal_string(hex: &str, field: &str) -> AppResult<String> {
    let digits = normalize_hex(hex, 64, field)?;
    BigUint::parse_bytes(digits.as_bytes(), 16)
        .map(|value| value.to_string())
        .ok_or_else(|| AppError::Validation(format!("invalid evm {field} {hex}")))
}

pub fn wei_to_decimal_string(raw: &str, decimals: i32) -> AppResult<String> {
    if decimals < 0 {
        return Err(AppError::Validation(
            "asset decimals cannot be negative".to_string(),
        ));
    }
    let _ = BigUint::from_str(raw)
        .map_err(|error| AppError::Validation(format!("invalid decimal balance {raw}: {error}")))?;
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

pub fn address_to_topic(address: &str) -> AppResult<String> {
    let address = normalize_hex(address, 40, "address")?.to_ascii_lowercase();
    Ok(format!("0x000000000000000000000000{address}"))
}

pub fn topic_to_address(topic: &str) -> AppResult<String> {
    let topic = normalize_hex(topic, 64, "topic")?;
    if !topic[..24].chars().all(|character| character == '0') {
        return Err(AppError::Validation(format!(
            "invalid evm address topic {topic}: non-zero padding"
        )));
    }
    Ok(format!("0x{}", &topic[24..]))
}

pub fn decode_erc20_transfer_log(
    log: &EvmLog,
    token_decimals: i32,
) -> AppResult<DecodedErc20Transfer> {
    if log.topics.len() < 3 {
        return Err(AppError::Validation(format!(
            "invalid erc20 transfer log topic count: {}",
            log.topics.len()
        )));
    }
    if log.topics[0].to_lowercase() != TRANSFER_TOPIC0 {
        return Err(AppError::Validation(
            "invalid erc20 transfer log topic0".to_string(),
        ));
    }

    let tx_hash = log.transaction_hash.clone().ok_or_else(|| {
        AppError::Validation("invalid erc20 transfer log: missing transactionHash".to_string())
    })?;
    normalize_hex(&tx_hash, 64, "transactionHash")?;
    if let Some(block_hash) = &log.block_hash {
        normalize_hex(block_hash, 64, "blockHash")?;
    }
    let log_index = parse_required_hex_i32(log.log_index.as_deref(), "logIndex")?;
    let block_number = parse_required_hex_i64(log.block_number.as_deref(), "blockNumber")?;
    let amount_raw = parse_abi_u256_to_decimal_string(&log.data, "data")?;
    let amount_decimal = wei_to_decimal_string(&amount_raw, token_decimals)?;
    let token_contract = normalize_hex(&log.address, 40, "address")?.to_ascii_lowercase();

    Ok(DecodedErc20Transfer {
        tx_hash,
        log_index,
        block_number,
        block_hash: log.block_hash.clone(),
        from_address: topic_to_address(&log.topics[1])?,
        to_address: topic_to_address(&log.topics[2])?,
        amount_raw,
        amount_decimal,
        token_contract: Some(format!("0x{token_contract}")),
    })
}

pub fn transfer_event_draft(
    context: &ScanAddressContext,
    asset: &Asset,
    transfer: DecodedErc20Transfer,
) -> AddressEventDraft {
    let watched = context.address.to_lowercase();
    let from = transfer.from_address.to_lowercase();
    let to = transfer.to_address.to_lowercase();
    let direction = if from == watched && to == watched {
        "self"
    } else if to == watched {
        "in"
    } else if from == watched {
        "out"
    } else {
        "unknown"
    };

    AddressEventDraft {
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: asset.id,
        event_type: "transfer".to_string(),
        direction: direction.to_string(),
        is_transfer: true,
        tx_hash: Some(transfer.tx_hash),
        log_index: Some(transfer.log_index),
        block_number: Some(transfer.block_number),
        block_hash: transfer.block_hash,
        confirmations: 0,
        from_address: Some(transfer.from_address),
        to_address: Some(transfer.to_address),
        amount_raw: Some(transfer.amount_raw),
        amount_decimal: Some(transfer.amount_decimal),
        balance_before_raw: None,
        balance_after_raw: None,
        balance_delta_raw: None,
        metadata: json!({
            "source": "evm_erc20_transfer_log",
            "token_contract": transfer.token_contract,
        }),
    }
}

pub fn evm_balance_change_event(
    context: &ScanAddressContext,
    asset: &Asset,
    previous: &BalanceSnapshot,
    current: &BalanceSnapshot,
    provider: &Provider,
) -> AppResult<AddressEventDraft> {
    let previous_raw = parse_decimal_bigint(&previous.balance_raw)?;
    let current_raw = parse_decimal_bigint(&current.balance_raw)?;
    let delta = current_raw - previous_raw;
    let direction = if delta.sign() == num_bigint::Sign::Minus {
        "out"
    } else {
        "in"
    };

    Ok(AddressEventDraft {
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: asset.id,
        event_type: "balance_change".to_string(),
        direction: direction.to_string(),
        is_transfer: false,
        tx_hash: None,
        log_index: None,
        block_number: current.block_number,
        block_hash: current.block_hash.clone(),
        confirmations: 0,
        from_address: None,
        to_address: None,
        amount_raw: None,
        amount_decimal: None,
        balance_before_raw: Some(previous.balance_raw.clone()),
        balance_after_raw: Some(current.balance_raw.clone()),
        balance_delta_raw: Some(delta.to_string()),
        metadata: json!({
            "source": "evm_balance_snapshot",
            "provider_id": provider.id,
            "provider_name": provider.name,
            "previous_snapshot_id": previous.id,
            "current_snapshot_id": current.id,
            "source_provider_id": current.source_provider_id,
            "block_number": current.block_number,
        }),
    })
}

fn parse_decimal_bigint(raw: &str) -> AppResult<BigInt> {
    BigInt::from_str(raw)
        .map_err(|error| AppError::Validation(format!("invalid decimal balance {raw}: {error}")))
}

fn hex_digits(hex: &str) -> AppResult<&str> {
    let digits = hex.strip_prefix("0x").ok_or_else(|| {
        AppError::Validation(format!("invalid hex quantity {hex}: missing 0x prefix"))
    })?;
    if digits.is_empty()
        || !digits
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(AppError::Validation(format!("invalid hex quantity {hex}")));
    }
    Ok(digits)
}

fn normalize_hex<'a>(value: &'a str, expected_len: usize, kind: &str) -> AppResult<&'a str> {
    let digits = value.strip_prefix("0x").ok_or_else(|| {
        AppError::Validation(format!("invalid evm {kind} {value}: missing 0x prefix"))
    })?;
    if digits.len() != expected_len
        || !digits
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(AppError::Validation(format!("invalid evm {kind} {value}")));
    }
    Ok(digits)
}

fn parse_required_hex_i64(value: Option<&str>, field: &str) -> AppResult<i64> {
    let value = value.ok_or_else(|| {
        AppError::Validation(format!("invalid erc20 transfer log: missing {field}"))
    })?;
    parse_hex_quantity_to_i64(value)
}

fn parse_required_hex_i32(value: Option<&str>, field: &str) -> AppResult<i32> {
    let value = parse_required_hex_i64(value, field)?;
    i32::try_from(value).map_err(|error| {
        AppError::Validation(format!("invalid erc20 transfer log {field}: {error}"))
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvmTransfer {
    pub tx_hash: String,
    pub log_index: Option<i32>,
    pub block_number: i64,
    pub block_hash: Option<String>,
    pub from_address: String,
    pub to_address: String,
    pub amount_raw: String,
    pub amount_decimal: String,
    pub token_contract: Option<String>,
}

pub fn classify_evm_transfer(
    address: &WatchedAddress,
    asset: &Asset,
    transfer: RawEvmTransfer,
) -> AddressEventDraft {
    let watched = address.address.to_lowercase();
    let from = transfer.from_address.to_lowercase();
    let to = transfer.to_address.to_lowercase();
    let direction = if from == watched && to == watched {
        "self"
    } else if to == watched {
        "in"
    } else if from == watched {
        "out"
    } else {
        "unknown"
    };

    AddressEventDraft {
        tenant_id: address.tenant_id,
        chain_id: address.chain_id,
        address_id: address.id,
        asset_id: asset.id,
        event_type: "transfer".to_string(),
        direction: direction.to_string(),
        is_transfer: true,
        tx_hash: Some(transfer.tx_hash),
        log_index: transfer.log_index,
        block_number: Some(transfer.block_number),
        block_hash: transfer.block_hash,
        confirmations: 0,
        from_address: Some(transfer.from_address),
        to_address: Some(transfer.to_address),
        amount_raw: Some(transfer.amount_raw),
        amount_decimal: Some(transfer.amount_decimal),
        balance_before_raw: None,
        balance_after_raw: None,
        balance_delta_raw: None,
        metadata: json!({
            "source": "mock_evm_transfer",
            "token_contract": transfer.token_contract,
        }),
    }
}

pub fn mock_evm_transfer(
    address: &WatchedAddress,
    asset: &Asset,
    sequence: i64,
) -> AddressEventDraft {
    let from_address = "0x0000000000000000000000000000000000000001".to_string();
    let to_address = address.address.clone();

    classify_evm_transfer(
        address,
        asset,
        RawEvmTransfer {
            tx_hash: mock_hash("tx", address, asset, sequence),
            log_index: Some(0),
            block_number: sequence,
            block_hash: Some(mock_hash("block", address, asset, sequence)),
            from_address,
            to_address,
            amount_raw: "1000000000000000000".to_string(),
            amount_decimal: "1.0".to_string(),
            token_contract: asset.contract_address.clone(),
        },
    )
}

fn mock_hash(kind: &str, address: &WatchedAddress, asset: &Asset, sequence: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update(address.id.as_bytes());
    hasher.update(address.chain_id.as_bytes());
    hasher.update(asset.id.as_bytes());
    hasher.update(sequence.to_be_bytes());
    format!("0x{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::{
        address_to_topic, build_json_rpc_request, decode_erc20_transfer_log,
        evm_balance_change_event, format_rpc_request_error, mock_evm_transfer,
        parse_hex_quantity_to_i64, parse_hex_u256_to_decimal_string, parse_json_rpc_hex_result,
        topic_to_address, transfer_event_draft, wei_to_decimal_string, DecodedErc20Transfer,
        EvmBlockTag, EvmLog, EvmLogFilter, TRANSFER_TOPIC0,
    };
    use coin_listener_core::{
        models::{Asset, BalanceSnapshot, Provider, ScanAddressContext, WatchedAddress},
        AppError,
    };
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn json_rpc_request_body_uses_method_params_and_jsonrpc_version() {
        let request = build_json_rpc_request(
            "eth_getBalance",
            json!([
                "0x1111111111111111111111111111111111111111",
                EvmBlockTag::Latest.as_param()
            ]),
        );
        assert_eq!(request["jsonrpc"], "2.0");
        assert_eq!(request["id"], 1);
        assert_eq!(request["method"], "eth_getBalance");
        assert_eq!(
            request["params"][0],
            "0x1111111111111111111111111111111111111111"
        );
        assert_eq!(request["params"][1], "latest");
    }

    #[test]
    fn transfer_topic_and_address_topic_encoding_are_stable() {
        assert_eq!(
            TRANSFER_TOPIC0,
            "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
        );
        assert_eq!(
            address_to_topic("0x1111111111111111111111111111111111111111").unwrap(),
            "0x0000000000000000000000001111111111111111111111111111111111111111"
        );
        assert_eq!(
            address_to_topic("0xABCDEFabcdefABCDEFabcdefABCDEFabcdefABCD").unwrap(),
            "0x000000000000000000000000abcdefabcdefabcdefabcdefabcdefabcdefabcd"
        );
        assert_eq!(
            topic_to_address("0x0000000000000000000000001111111111111111111111111111111111111111")
                .unwrap(),
            "0x1111111111111111111111111111111111111111"
        );
    }

    #[test]
    fn eth_get_logs_request_body_contains_range_address_and_topics() {
        let filter = EvmLogFilter {
            address: "0xdac17f958d2ee523a2206206994597c13d831ec7".to_string(),
            from_block: 20_000_000,
            to_block: 20_000_010,
            topics: vec![
                Some(TRANSFER_TOPIC0.to_string()),
                None,
                Some(address_to_topic("0x1111111111111111111111111111111111111111").unwrap()),
            ],
        };

        let request = build_json_rpc_request("eth_getLogs", filter.to_rpc_params().unwrap());

        assert_eq!(request["method"], "eth_getLogs");
        assert_eq!(
            request["params"][0]["address"],
            "0xdac17f958d2ee523a2206206994597c13d831ec7"
        );
        assert_eq!(request["params"][0]["fromBlock"], "0x1312d00");
        assert_eq!(request["params"][0]["toBlock"], "0x1312d0a");
        assert_eq!(request["params"][0]["topics"][0], TRANSFER_TOPIC0);
        assert!(request["params"][0]["topics"][1].is_null());
    }

    #[test]
    fn erc20_transfer_log_decodes_to_transfer_fields() {
        let tx_hash = evm_hash(1);
        let block_hash = evm_hash(2);
        let log = EvmLog {
            address: "0xDAC17F958D2EE523A2206206994597C13D831EC7".to_string(),
            topics: vec![
                TRANSFER_TOPIC0.to_string(),
                address_to_topic("0x2222222222222222222222222222222222222222").unwrap(),
                address_to_topic("0x1111111111111111111111111111111111111111").unwrap(),
                "0x0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            ],
            data: "0x00000000000000000000000000000000000000000000000000000000000f4240".to_string(),
            transaction_hash: Some(tx_hash.clone()),
            log_index: Some("0x2".to_string()),
            block_number: Some("0x1312d00".to_string()),
            block_hash: Some(block_hash.clone()),
        };

        let decoded = decode_erc20_transfer_log(&log, 6).unwrap();

        assert_eq!(decoded.tx_hash, tx_hash);
        assert_eq!(decoded.log_index, 2);
        assert_eq!(decoded.block_number, 20_000_000);
        assert_eq!(decoded.block_hash, Some(block_hash));
        assert_eq!(
            decoded.from_address,
            "0x2222222222222222222222222222222222222222"
        );
        assert_eq!(
            decoded.to_address,
            "0x1111111111111111111111111111111111111111"
        );
        assert_eq!(decoded.amount_raw, "1000000");
        assert_eq!(decoded.amount_decimal, "1.0");
        assert_eq!(
            decoded.token_contract,
            Some("0xdac17f958d2ee523a2206206994597c13d831ec7".to_string())
        );
    }

    #[test]
    fn evm_log_filter_rejects_invalid_block_ranges() {
        let mut filter = transfer_filter(20_000_000, 20_000_010);

        filter.from_block = -1;
        assert!(matches!(
            filter.to_rpc_params(),
            Err(AppError::Validation(message)) if message.contains("fromBlock")
        ));

        filter.from_block = 20_000_011;
        assert!(matches!(
            filter.to_rpc_params(),
            Err(AppError::Validation(message)) if message.contains("fromBlock") && message.contains("toBlock")
        ));
    }

    #[test]
    fn erc20_transfer_log_rejects_missing_indexed_topics_with_validation_error() {
        let mut log = transfer_log();
        log.topics = vec![
            TRANSFER_TOPIC0.to_string(),
            address_to_topic("0x2222222222222222222222222222222222222222").unwrap(),
        ];

        let result = decode_erc20_transfer_log(&log, 6);

        assert!(
            matches!(result, Err(AppError::Validation(message)) if message.contains("topic count"))
        );
    }

    #[test]
    fn erc20_transfer_log_rejects_malformed_abi_fields() {
        let mut short_data = transfer_log();
        short_data.data = "0x1".to_string();
        assert!(matches!(
            decode_erc20_transfer_log(&short_data, 6),
            Err(AppError::Validation(message)) if message.contains("data")
        ));

        let mut bad_tx_hash = transfer_log();
        bad_tx_hash.transaction_hash = Some("0xtxhash".to_string());
        assert!(matches!(
            decode_erc20_transfer_log(&bad_tx_hash, 6),
            Err(AppError::Validation(message)) if message.contains("transactionHash")
        ));

        let mut bad_block_hash = transfer_log();
        bad_block_hash.block_hash = Some("0xblockhash".to_string());
        assert!(matches!(
            decode_erc20_transfer_log(&bad_block_hash, 6),
            Err(AppError::Validation(message)) if message.contains("blockHash")
        ));

        let mut bad_contract = transfer_log();
        bad_contract.address = "0xnot-an-address".to_string();
        assert!(matches!(
            decode_erc20_transfer_log(&bad_contract, 6),
            Err(AppError::Validation(message)) if message.contains("address")
        ));
    }

    #[test]
    fn transfer_event_draft_uses_scan_context_for_inbound_transfer() {
        let context = scan_context();
        let asset = native_asset(context.chain_id);
        let transfer = decoded_transfer(
            "0x2222222222222222222222222222222222222222",
            &context.address,
        );

        let draft = transfer_event_draft(&context, &asset, transfer);

        assert_eq!(draft.tenant_id, context.tenant_id);
        assert_eq!(draft.chain_id, context.chain_id);
        assert_eq!(draft.address_id, context.id);
        assert_eq!(draft.direction, "in");
        assert_eq!(draft.metadata["source"], "evm_erc20_transfer_log");
    }

    #[test]
    fn json_rpc_response_parser_returns_hex_result() {
        let payload = r#"{"jsonrpc":"2.0","id":1,"result":"0xde0b6b3a7640000"}"#;
        let result = parse_json_rpc_hex_result(payload, "eth_getBalance").unwrap();
        assert_eq!(result, "0xde0b6b3a7640000");
    }

    #[test]
    fn json_rpc_response_parser_rejects_error_payload() {
        let payload =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"execution reverted"}}"#;
        let result = parse_json_rpc_hex_result(payload, "eth_getBalance");
        assert!(matches!(
            result,
            Err(AppError::Validation(message))
                if message.contains("eth_getBalance") && message.contains("execution reverted")
        ));
    }

    #[test]
    fn json_rpc_request_errors_do_not_include_provider_url() {
        let message = format_rpc_request_error(
            "eth_getBalance",
            "https://example.invalid/provider-key",
            "connection refused",
        );
        assert!(message.contains("eth_getBalance"));
        assert!(message.contains("connection refused"));
        assert!(!message.contains("example.invalid"));
        assert!(!message.contains("provider-key"));
    }

    #[test]
    fn hex_quantity_parsing_supports_block_numbers_and_large_balances() {
        assert_eq!(parse_hex_quantity_to_i64("0x0").unwrap(), 0);
        assert_eq!(parse_hex_quantity_to_i64("0x1").unwrap(), 1);
        assert_eq!(
            parse_hex_u256_to_decimal_string("0xde0b6b3a7640000").unwrap(),
            "1000000000000000000"
        );
    }

    #[test]
    fn invalid_hex_quantity_returns_validation_error() {
        let result = parse_hex_u256_to_decimal_string("0xnothex");
        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message.contains("invalid hex quantity")
        ));
    }

    #[test]
    fn wei_decimal_formatting_respects_asset_decimals() {
        assert_eq!(
            wei_to_decimal_string("1000000000000000000", 18).unwrap(),
            "1.0"
        );
        assert_eq!(
            wei_to_decimal_string("1", 18).unwrap(),
            "0.000000000000000001"
        );
        assert_eq!(wei_to_decimal_string("123450000", 6).unwrap(), "123.45");
        assert_eq!(wei_to_decimal_string("1000", 0).unwrap(), "1000");
    }

    #[test]
    fn mock_evm_transfer_uses_evm_shaped_hashes_and_sequence() {
        let chain_id = Uuid::new_v4();
        let address = WatchedAddress {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            chain_id,
            address: "0x1111111111111111111111111111111111111111".to_string(),
            label: None,
            priority: "normal".to_string(),
            scan_interval_seconds: 300,
            transfer_filter_enabled: true,
            balance_change_filter_enabled: true,
            status: "active".to_string(),
        };
        let asset = Asset {
            id: Uuid::new_v4(),
            chain_id,
            asset_type: "native".to_string(),
            symbol: "ETH".to_string(),
            name: "Ether".to_string(),
            contract_address: None,
            decimals: 18,
            is_builtin: true,
            status: "active".to_string(),
        };

        let first = mock_evm_transfer(&address, &asset, 1);
        let second = mock_evm_transfer(&address, &asset, 2);

        let first_hash = first.tx_hash.expect("tx hash");
        let first_block_hash = first.block_hash.expect("block hash");
        assert_eq!(first_hash.len(), 66);
        assert_eq!(first_block_hash.len(), 66);
        assert!(first_hash.starts_with("0x"));
        assert!(first_hash[2..]
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
        assert_ne!(first_hash, second.tx_hash.expect("second tx hash"));
        assert_eq!(first.block_number, Some(1));
        assert_eq!(second.block_number, Some(2));
    }

    #[test]
    fn balance_change_event_marks_inbound_balance_increase() {
        let context = scan_context();
        let asset = native_asset(context.chain_id);
        let provider = rpc_provider(context.chain_id);
        let previous = snapshot(Uuid::from_u128(401), &context, &asset, "100", 20_000_000);
        let current = snapshot(Uuid::from_u128(402), &context, &asset, "150", 20_000_001);

        let event =
            evm_balance_change_event(&context, &asset, &previous, &current, &provider).unwrap();

        assert_eq!(event.tenant_id, context.tenant_id);
        assert_eq!(event.chain_id, context.chain_id);
        assert_eq!(event.address_id, context.id);
        assert_eq!(event.asset_id, asset.id);
        assert_eq!(event.direction, "in");
        assert_eq!(event.event_type, "balance_change");
        assert!(!event.is_transfer);
        assert_eq!(event.tx_hash, None);
        assert_eq!(event.log_index, None);
        assert_eq!(event.from_address, None);
        assert_eq!(event.to_address, None);
        assert_eq!(event.amount_raw, None);
        assert_eq!(event.amount_decimal, None);
        assert_eq!(event.block_number, current.block_number);
        assert_eq!(event.block_hash, current.block_hash);
        assert_eq!(event.confirmations, 0);
        assert_eq!(event.balance_before_raw, Some("100".to_string()));
        assert_eq!(event.balance_after_raw, Some("150".to_string()));
        assert_eq!(event.balance_delta_raw, Some("50".to_string()));
        assert_eq!(event.metadata["source"], "evm_balance_snapshot");
        assert_eq!(event.metadata["provider_name"], "Primary RPC");
    }

    #[test]
    fn balance_change_event_marks_outbound_balance_decrease() {
        let context = scan_context();
        let asset = native_asset(context.chain_id);
        let provider = rpc_provider(context.chain_id);
        let previous = snapshot(Uuid::from_u128(401), &context, &asset, "150", 20_000_000);
        let current = snapshot(Uuid::from_u128(402), &context, &asset, "100", 20_000_001);

        let event =
            evm_balance_change_event(&context, &asset, &previous, &current, &provider).unwrap();

        assert_eq!(event.direction, "out");
        assert_eq!(event.balance_delta_raw, Some("-50".to_string()));
    }

    fn scan_context() -> ScanAddressContext {
        ScanAddressContext {
            id: Uuid::from_u128(101),
            tenant_id: Uuid::from_u128(102),
            chain_id: Uuid::from_u128(103),
            address: "0x1111111111111111111111111111111111111111".to_string(),
            scan_interval_seconds: 300,
            chain_type: "evm".to_string(),
        }
    }

    fn native_asset(chain_id: Uuid) -> Asset {
        Asset {
            id: Uuid::from_u128(201),
            chain_id,
            asset_type: "native".to_string(),
            symbol: "ETH".to_string(),
            name: "Ether".to_string(),
            contract_address: None,
            decimals: 18,
            is_builtin: true,
            status: "active".to_string(),
        }
    }

    fn rpc_provider(chain_id: Uuid) -> Provider {
        Provider {
            id: Uuid::from_u128(301),
            chain_id,
            provider_type: "rpc".to_string(),
            name: "Primary RPC".to_string(),
            base_url: "https://example.invalid".to_string(),
            api_key_ref: None,
            priority: 1,
            qps_limit: 10,
            timeout_ms: 5000,
            status: "active".to_string(),
        }
    }

    fn transfer_filter(from_block: i64, to_block: i64) -> EvmLogFilter {
        EvmLogFilter {
            address: "0xdac17f958d2ee523a2206206994597c13d831ec7".to_string(),
            from_block,
            to_block,
            topics: vec![
                Some(TRANSFER_TOPIC0.to_string()),
                None,
                Some(address_to_topic("0x1111111111111111111111111111111111111111").unwrap()),
            ],
        }
    }

    fn transfer_log() -> EvmLog {
        EvmLog {
            address: "0xdac17f958d2ee523a2206206994597c13d831ec7".to_string(),
            topics: vec![
                TRANSFER_TOPIC0.to_string(),
                address_to_topic("0x2222222222222222222222222222222222222222").unwrap(),
                address_to_topic("0x1111111111111111111111111111111111111111").unwrap(),
            ],
            data: "0x00000000000000000000000000000000000000000000000000000000000f4240".to_string(),
            transaction_hash: Some(evm_hash(1)),
            log_index: Some("0x2".to_string()),
            block_number: Some("0x1312d00".to_string()),
            block_hash: Some(evm_hash(2)),
        }
    }

    fn evm_hash(value: u8) -> String {
        format!("0x{value:064x}")
    }

    fn decoded_transfer(from_address: &str, to_address: &str) -> DecodedErc20Transfer {
        DecodedErc20Transfer {
            tx_hash: evm_hash(1),
            log_index: 2,
            block_number: 20_000_000,
            block_hash: Some(evm_hash(2)),
            from_address: from_address.to_string(),
            to_address: to_address.to_string(),
            amount_raw: "1000000".to_string(),
            amount_decimal: "1.0".to_string(),
            token_contract: Some("0xdac17f958d2ee523a2206206994597c13d831ec7".to_string()),
        }
    }

    fn snapshot(
        id: Uuid,
        context: &ScanAddressContext,
        asset: &Asset,
        raw: &str,
        block_number: i64,
    ) -> BalanceSnapshot {
        BalanceSnapshot {
            id,
            tenant_id: context.tenant_id,
            chain_id: context.chain_id,
            address_id: context.id,
            asset_id: asset.id,
            balance_raw: raw.to_string(),
            balance_decimal: wei_to_decimal_string(raw, 18).unwrap(),
            block_number: Some(block_number),
            block_hash: Some(format!("0x{block_number:064x}")),
            observed_at: "2026-05-17T10:00:00Z".parse().unwrap(),
            source_provider_id: Some(Uuid::from_u128(301)),
        }
    }
}
