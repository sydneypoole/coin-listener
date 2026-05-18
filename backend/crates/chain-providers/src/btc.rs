use coin_listener_core::{
    models::{AddressEventDraft, Asset, ScanAddressContext},
    AppError, AppResult,
};
use num_bigint::BigInt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{str::FromStr, time::Duration};

const BTC_DECIMALS: i32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtcBalance {
    pub balance_raw: String,
    pub balance_decimal: String,
}

#[derive(Debug, Clone)]
pub struct BtcTransactionPage {
    pub transactions: Vec<BtcTransaction>,
    pub next_last_seen_txid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBtcTransfer {
    pub tx_hash: String,
    pub block_number: i64,
    pub block_hash: Option<String>,
    pub direction: String,
    pub amount_raw: String,
    pub amount_decimal: String,
    pub received_raw: String,
    pub spent_raw: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BtcTransaction {
    pub txid: String,
    pub status: BtcTxStatus,
    #[serde(default)]
    pub vin: Vec<BtcVin>,
    #[serde(default)]
    pub vout: Vec<BtcVout>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BtcTxStatus {
    pub confirmed: bool,
    pub block_height: Option<i64>,
    pub block_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BtcVin {
    pub prevout: Option<BtcVout>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BtcVout {
    pub scriptpubkey_address: Option<String>,
    pub value: i64,
}

pub fn normalize_btc_address(address: &str) -> AppResult<String> {
    let address = address.trim();
    let has_supported_prefix = address.starts_with("bc1")
        || address.starts_with("tb1")
        || address.starts_with('1')
        || address.starts_with('3');
    let has_valid_shape = (14..=90).contains(&address.len())
        && has_supported_prefix
        && address
            .chars()
            .all(|character| character.is_ascii_alphanumeric());

    if has_valid_shape {
        Ok(address.to_string())
    } else {
        Err(AppError::Validation(format!(
            "invalid btc address {address}"
        )))
    }
}

pub fn decode_btc_confirmed_balance(payload: &Value) -> AppResult<BtcBalance> {
    let funded = parse_non_negative_json_bigint(payload, &["chain_stats", "funded_txo_sum"])?;
    let spent = parse_non_negative_json_bigint(payload, &["chain_stats", "spent_txo_sum"])?;
    let balance = funded - spent;
    if balance < BigInt::from(0) {
        return Err(AppError::Validation(
            "invalid btc confirmed balance: spent_txo_sum exceeds funded_txo_sum".to_string(),
        ));
    }

    let balance_raw = balance.to_string();
    let balance_decimal = sats_to_decimal_string(&balance_raw)?;
    Ok(BtcBalance {
        balance_raw,
        balance_decimal,
    })
}

#[derive(Debug, Clone)]
pub struct BtcClient {
    base_url: String,
    client: reqwest::Client,
}

impl BtcClient {
    pub fn new(base_url: String, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("valid BTC client");
        Self { base_url, client }
    }

    pub fn address_path(&self, address: &str) -> AppResult<String> {
        let address = normalize_btc_address(address)?;
        Ok(format!("/address/{address}"))
    }

    pub fn address_txs_path(&self, address: &str) -> AppResult<String> {
        self.address_txs_page_path(address, None)
    }

    pub fn address_txs_page_path(
        &self,
        address: &str,
        last_seen_txid: Option<&str>,
    ) -> AppResult<String> {
        let address = normalize_btc_address(address)?;
        match last_seen_txid {
            Some(txid) => {
                validate_btc_hash(txid, "last_seen_txid")?;
                Ok(format!("/address/{address}/txs/chain/{txid}"))
            }
            None => Ok(format!("/address/{address}/txs/chain")),
        }
    }

    pub async fn address_balance(&self, address: &str) -> AppResult<BtcBalance> {
        let path = self.address_path(address)?;
        let body = self.get_json_body("address balance", &path).await?;
        decode_btc_confirmed_balance(&body)
    }

    pub async fn address_transactions(&self, address: &str) -> AppResult<Vec<BtcTransaction>> {
        self.address_transactions_page(address, None)
            .await
            .map(|page| page.transactions)
    }

    pub async fn address_transactions_page(
        &self,
        address: &str,
        last_seen_txid: Option<&str>,
    ) -> AppResult<BtcTransactionPage> {
        let path = self.address_txs_page_path(address, last_seen_txid)?;
        let body = self.get_json_body("address transactions", &path).await?;
        decode_btc_transaction_page(body)
    }

    async fn get_json_body(&self, operation: &str, path: &str) -> AppResult<Value> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let response = self.client.get(&url).send().await.map_err(|error| {
            AppError::Config(format_btc_request_error(
                operation,
                &self.base_url,
                &error.without_url().to_string(),
            ))
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            AppError::Config(format!(
                "BTC {operation} response body failed: {}",
                error.without_url()
            ))
        })?;
        if !status.is_success() {
            return Err(AppError::Config(format_btc_status_error(
                operation,
                &self.base_url,
                status,
                &body,
            )));
        }
        serde_json::from_str(&body).map_err(|error| {
            AppError::Validation(format!("invalid BTC {operation} response json: {error}"))
        })
    }
}

pub fn decode_btc_transaction_page(body: Value) -> AppResult<BtcTransactionPage> {
    let transactions: Vec<BtcTransaction> = serde_json::from_value(body).map_err(|error| {
        AppError::Validation(format!(
            "invalid BTC address transactions response json: {error}"
        ))
    })?;
    let next_last_seen_txid = transactions
        .last()
        .map(|transaction| transaction.txid.clone());
    if let Some(txid) = next_last_seen_txid.as_deref() {
        validate_btc_hash(txid, "last_seen_txid")?;
    }

    Ok(BtcTransactionPage {
        transactions,
        next_last_seen_txid,
    })
}

pub fn format_btc_request_error(operation: &str, base_url: &str, error: &str) -> String {
    format!(
        "BTC {operation} request failed: {}",
        redact_provider_url(error, base_url)
    )
}

pub fn format_btc_status_error(
    operation: &str,
    base_url: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> String {
    format!(
        "BTC {operation} returned http {status}: {}",
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

pub fn classify_btc_transaction(
    tx: &BtcTransaction,
    watched_address: &str,
) -> AppResult<Option<DecodedBtcTransfer>> {
    let watched_address = normalize_btc_address(watched_address)?;
    if !tx.status.confirmed {
        return Err(AppError::Validation(
            "invalid btc transaction: status.confirmed must be true".to_string(),
        ));
    }

    let block_number = tx.status.block_height.ok_or_else(|| {
        AppError::Validation("invalid btc transaction: missing block_height".to_string())
    })?;
    if block_number < 0 {
        return Err(AppError::Validation(format!(
            "invalid btc transaction block_height {block_number}: must be non-negative"
        )));
    }

    validate_btc_hash(&tx.txid, "txid")?;
    if let Some(block_hash) = tx.status.block_hash.as_deref() {
        validate_btc_hash(block_hash, "block_hash")?;
    }

    let mut received = BigInt::from(0);
    for output in &tx.vout {
        validate_non_negative_sats(output.value, "vout.value")?;
        if output.scriptpubkey_address.as_deref() == Some(watched_address.as_str()) {
            received += BigInt::from(output.value);
        }
    }

    let mut spent = BigInt::from(0);
    for input in &tx.vin {
        if let Some(prevout) = &input.prevout {
            validate_non_negative_sats(prevout.value, "vin.prevout.value")?;
            if prevout.scriptpubkey_address.as_deref() == Some(watched_address.as_str()) {
                spent += BigInt::from(prevout.value);
            }
        }
    }

    if received == BigInt::from(0) && spent == BigInt::from(0) {
        return Ok(None);
    }

    let delta = &received - &spent;
    let direction = if delta > BigInt::from(0) {
        "in"
    } else if delta < BigInt::from(0) {
        "out"
    } else {
        "self"
    };
    let amount = if delta < BigInt::from(0) {
        -delta
    } else {
        delta
    };
    let amount_raw = amount.to_string();
    let amount_decimal = sats_to_decimal_string(&amount_raw)?;

    Ok(Some(DecodedBtcTransfer {
        tx_hash: tx.txid.clone(),
        block_number,
        block_hash: tx.status.block_hash.clone(),
        direction: direction.to_string(),
        amount_raw,
        amount_decimal,
        received_raw: received.to_string(),
        spent_raw: spent.to_string(),
    }))
}

pub fn btc_transfer_event_draft(
    context: &ScanAddressContext,
    asset: &Asset,
    transfer: DecodedBtcTransfer,
) -> AddressEventDraft {
    AddressEventDraft {
        tenant_id: context.tenant_id,
        chain_id: context.chain_id,
        address_id: context.id,
        asset_id: asset.id,
        event_type: "transfer".to_string(),
        direction: transfer.direction,
        is_transfer: true,
        tx_hash: Some(transfer.tx_hash),
        log_index: None,
        block_number: Some(transfer.block_number),
        block_hash: transfer.block_hash,
        confirmations: 0,
        from_address: None,
        to_address: None,
        amount_raw: Some(transfer.amount_raw),
        amount_decimal: Some(transfer.amount_decimal),
        balance_before_raw: None,
        balance_after_raw: None,
        balance_delta_raw: None,
        metadata: json!({
            "source": "btc_transaction",
            "received_raw": transfer.received_raw,
            "spent_raw": transfer.spent_raw,
        }),
    }
}

pub fn sats_to_decimal_string(raw: &str) -> AppResult<String> {
    raw_to_decimal_string(raw, BTC_DECIMALS)
}

fn raw_to_decimal_string(raw: &str, decimals: i32) -> AppResult<String> {
    if decimals < 0 {
        return Err(AppError::Validation(
            "invalid decimal scale: must be non-negative".to_string(),
        ));
    }

    let raw = raw.trim();
    let parsed = BigInt::from_str(raw)
        .map_err(|error| AppError::Validation(format!("invalid btc amount {raw}: {error}")))?;
    if parsed < BigInt::from(0) {
        return Err(AppError::Validation(format!(
            "invalid btc amount {raw}: must be non-negative"
        )));
    }

    let digits = parsed.to_string();
    let decimals = decimals as usize;
    if decimals == 0 {
        return Ok(digits);
    }

    let padded = if digits.len() <= decimals {
        format!("{:0>width$}", digits, width = decimals + 1)
    } else {
        digits
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

fn parse_non_negative_json_bigint(payload: &Value, path: &[&str]) -> AppResult<BigInt> {
    let value = path.iter().try_fold(payload, |current, key| {
        current.get(*key).ok_or_else(|| {
            AppError::Validation(format!(
                "invalid btc balance payload: missing {}",
                path.join(".")
            ))
        })
    })?;
    let parsed = match value {
        Value::Number(number) => BigInt::from_str(&number.to_string()),
        Value::String(text) => BigInt::from_str(text.trim()),
        _ => {
            return Err(AppError::Validation(format!(
                "invalid btc balance payload: {} must be an integer",
                path.join(".")
            )))
        }
    }
    .map_err(|error| {
        AppError::Validation(format!(
            "invalid btc balance payload {}: {error}",
            path.join(".")
        ))
    })?;

    if parsed < BigInt::from(0) {
        return Err(AppError::Validation(format!(
            "invalid btc balance payload: {} must be non-negative",
            path.join(".")
        )));
    }

    Ok(parsed)
}

fn validate_btc_hash(value: &str, field: &str) -> AppResult<()> {
    if value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "invalid btc transaction {field} {value}"
        )))
    }
}

fn validate_non_negative_sats(value: i64, field: &str) -> AppResult<()> {
    if value >= 0 {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "invalid btc transaction {field} {value}: must be non-negative"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        btc_transfer_event_draft, classify_btc_transaction, decode_btc_confirmed_balance,
        decode_btc_transaction_page, normalize_btc_address, BtcTransaction, DecodedBtcTransfer,
    };
    use coin_listener_core::{
        models::{Asset, ScanAddressContext},
        AppError,
    };
    use serde_json::{json, Value};
    use uuid::Uuid;

    const WATCHED: &str = "bc1qwatchedaddress000000000000000000000000";
    const OTHER: &str = "bc1qotheraddress00000000000000000000000000";

    #[test]
    fn btc_client_builds_address_paths_without_double_slashes() {
        let client = super::BtcClient::new(
            "https://mempool.space/api/".to_string(),
            std::time::Duration::from_secs(5),
        );
        let address = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";

        assert_eq!(
            client.address_path(address).unwrap(),
            format!("/address/{address}")
        );
        assert_eq!(
            client.address_txs_path(address).unwrap(),
            format!("/address/{address}/txs/chain")
        );
    }

    #[test]
    fn btc_client_builds_paginated_transaction_paths() {
        let client = super::BtcClient::new(
            "https://mempool.space/api/".to_string(),
            std::time::Duration::from_secs(5),
        );
        let address = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";
        let txid = btc_hash(9);

        assert_eq!(
            client.address_txs_page_path(address, None).unwrap(),
            format!("/address/{address}/txs/chain")
        );
        assert_eq!(
            client
                .address_txs_page_path(address, Some(txid.as_str()))
                .unwrap(),
            format!("/address/{address}/txs/chain/{txid}")
        );
    }

    #[test]
    fn btc_transaction_page_uses_last_txid_as_next_page_token() {
        let page =
            decode_btc_transaction_page(json!([btc_tx_json(1, 840_000), btc_tx_json(2, 840_001)]))
                .unwrap();

        assert_eq!(page.transactions.len(), 2);
        assert_eq!(page.next_last_seen_txid, Some(btc_hash(2)));
    }

    #[test]
    fn btc_transaction_page_without_transactions_has_no_next_token() {
        let page = decode_btc_transaction_page(json!([])).unwrap();

        assert!(page.transactions.is_empty());
        assert_eq!(page.next_last_seen_txid, None);
    }

    #[test]
    fn btc_status_errors_do_not_include_provider_url() {
        let message = super::format_btc_status_error(
            "address transactions",
            "https://btc.example.com/provider-key/",
            reqwest::StatusCode::BAD_GATEWAY,
            "upstream https://btc.example.com/provider-key/ failed",
        );

        assert!(message.contains("address transactions"));
        assert!(message.contains("502 Bad Gateway"));
        assert!(message.contains("[redacted provider url]"));
        assert!(!message.contains("btc.example.com"));
        assert!(!message.contains("provider-key"));
    }

    #[test]
    fn btc_request_errors_do_not_include_provider_url() {
        let message = super::format_btc_request_error(
            "address balance",
            "https://btc.example.com/provider-key/",
            "request to https://btc.example.com/provider-key failed: connection refused",
        );

        assert!(message.contains("address balance"));
        assert!(message.contains("connection refused"));
        assert!(!message.contains("btc.example.com"));
        assert!(!message.contains("provider-key"));
    }

    #[test]
    fn btc_address_normalization_accepts_supported_shapes_and_rejects_evm_hex() {
        assert_eq!(
            normalize_btc_address("  bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080  ").unwrap(),
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080"
        );
        assert_eq!(
            normalize_btc_address("1BoatSLRHtKNngkdXEeobR76b53LETtpyT").unwrap(),
            "1BoatSLRHtKNngkdXEeobR76b53LETtpyT"
        );
        assert_eq!(
            normalize_btc_address("3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy").unwrap(),
            "3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy"
        );

        assert!(matches!(
            normalize_btc_address("0x1111111111111111111111111111111111111111"),
            Err(AppError::Validation(message)) if message.contains("address")
        ));
    }

    #[test]
    fn btc_confirmed_balance_uses_chain_stats_only_and_ignores_mempool() {
        let payload = json!({
            "chain_stats": {
                "funded_txo_sum": 200_000_000,
                "spent_txo_sum": 50_000_000
            },
            "mempool_stats": {
                "funded_txo_sum": 900_000_000,
                "spent_txo_sum": 1
            }
        });

        let balance = decode_btc_confirmed_balance(&payload).unwrap();

        assert_eq!(balance.balance_raw, "150000000");
        assert_eq!(balance.balance_decimal, "1.5");
    }

    #[test]
    fn btc_transaction_classifies_inbound_delta() {
        let tx = btc_tx(
            vec![input(OTHER, 200_000_000)],
            vec![output(WATCHED, 123_456_789), output(OTHER, 76_543_211)],
        );

        let transfer = classify_btc_transaction(&tx, WATCHED).unwrap().unwrap();

        assert_eq!(transfer.tx_hash, btc_hash(1));
        assert_eq!(transfer.block_number, 840_000);
        assert_eq!(transfer.block_hash, Some(btc_hash(2)));
        assert_eq!(transfer.direction, "in");
        assert_eq!(transfer.received_raw, "123456789");
        assert_eq!(transfer.spent_raw, "0");
        assert_eq!(transfer.amount_raw, "123456789");
        assert_eq!(transfer.amount_decimal, "1.23456789");
    }

    #[test]
    fn btc_transaction_classifies_outbound_delta() {
        let tx = btc_tx(
            vec![input(WATCHED, 50_000)],
            vec![output(OTHER, 40_000), output(WATCHED, 10_000)],
        );

        let transfer = classify_btc_transaction(&tx, WATCHED).unwrap().unwrap();

        assert_eq!(transfer.direction, "out");
        assert_eq!(transfer.received_raw, "10000");
        assert_eq!(transfer.spent_raw, "50000");
        assert_eq!(transfer.amount_raw, "40000");
        assert_eq!(transfer.amount_decimal, "0.0004");
    }

    #[test]
    fn btc_transaction_classifies_self_zero_delta_when_watched_appears_on_both_sides() {
        let tx = btc_tx(vec![input(WATCHED, 25_000)], vec![output(WATCHED, 25_000)]);

        let transfer = classify_btc_transaction(&tx, WATCHED).unwrap().unwrap();

        assert_eq!(transfer.direction, "self");
        assert_eq!(transfer.received_raw, "25000");
        assert_eq!(transfer.spent_raw, "25000");
        assert_eq!(transfer.amount_raw, "0");
        assert_eq!(transfer.amount_decimal, "0.0");
    }

    #[test]
    fn btc_transaction_ignores_unrelated_tx() {
        let tx = btc_tx(vec![input(OTHER, 10_000)], vec![output(OTHER, 9_000)]);

        let transfer = classify_btc_transaction(&tx, WATCHED).unwrap();

        assert!(transfer.is_none());
    }

    #[test]
    fn btc_transaction_rejects_unconfirmed_tx() {
        let mut tx = btc_tx(vec![input(OTHER, 10_000)], vec![output(WATCHED, 9_000)]);
        tx.status.confirmed = false;

        let result = classify_btc_transaction(&tx, WATCHED);

        assert!(matches!(
            result,
            Err(AppError::Validation(message)) if message.contains("confirmed")
        ));
    }

    #[test]
    fn btc_transfer_event_draft_maps_unified_fields_and_metadata_source() {
        let context = scan_context(WATCHED);
        let asset = native_asset(context.chain_id);
        let transfer = DecodedBtcTransfer {
            tx_hash: btc_hash(1),
            block_number: 840_000,
            block_hash: Some(btc_hash(2)),
            direction: "in".to_string(),
            amount_raw: "123456789".to_string(),
            amount_decimal: "1.23456789".to_string(),
            received_raw: "123456789".to_string(),
            spent_raw: "0".to_string(),
        };

        let draft = btc_transfer_event_draft(&context, &asset, transfer);

        assert_eq!(draft.tenant_id, context.tenant_id);
        assert_eq!(draft.chain_id, context.chain_id);
        assert_eq!(draft.address_id, context.id);
        assert_eq!(draft.asset_id, asset.id);
        assert_eq!(draft.event_type, "transfer");
        assert_eq!(draft.direction, "in");
        assert!(draft.is_transfer);
        assert_eq!(draft.tx_hash, Some(btc_hash(1)));
        assert_eq!(draft.log_index, None);
        assert_eq!(draft.block_number, Some(840_000));
        assert_eq!(draft.block_hash, Some(btc_hash(2)));
        assert_eq!(draft.from_address, None);
        assert_eq!(draft.to_address, None);
        assert_eq!(draft.amount_raw, Some("123456789".to_string()));
        assert_eq!(draft.amount_decimal, Some("1.23456789".to_string()));
        assert_eq!(draft.metadata["source"], "btc_transaction");
        assert_eq!(draft.metadata["received_raw"], "123456789");
        assert_eq!(draft.metadata["spent_raw"], "0");
    }

    fn btc_tx(vin: Vec<Value>, vout: Vec<Value>) -> BtcTransaction {
        serde_json::from_value(json!({
            "txid": btc_hash(1),
            "status": {
                "confirmed": true,
                "block_height": 840_000,
                "block_hash": btc_hash(2)
            },
            "vin": vin,
            "vout": vout
        }))
        .unwrap()
    }

    fn btc_tx_json(seed: u8, block_height: i64) -> Value {
        json!({
            "txid": btc_hash(seed),
            "status": {
                "confirmed": true,
                "block_height": block_height,
                "block_hash": btc_hash(seed + 20)
            },
            "vin": [input(OTHER, 10_000)],
            "vout": [output(WATCHED, 9_000)]
        })
    }

    fn input(address: &str, value: i64) -> Value {
        json!({
            "prevout": {
                "scriptpubkey_address": address,
                "value": value
            }
        })
    }

    fn output(address: &str, value: i64) -> Value {
        json!({
            "scriptpubkey_address": address,
            "value": value
        })
    }

    fn scan_context(address: &str) -> ScanAddressContext {
        ScanAddressContext {
            id: Uuid::from_u128(101),
            tenant_id: Uuid::from_u128(102),
            chain_id: Uuid::from_u128(103),
            address: address.to_string(),
            scan_interval_seconds: 300,
            chain_type: "btc".to_string(),
        }
    }

    fn native_asset(chain_id: Uuid) -> Asset {
        Asset {
            id: Uuid::from_u128(201),
            chain_id,
            asset_type: "native".to_string(),
            symbol: "BTC".to_string(),
            name: "Bitcoin".to_string(),
            contract_address: None,
            decimals: 8,
            is_builtin: true,
            status: "active".to_string(),
        }
    }

    fn btc_hash(value: u8) -> String {
        format!("{value:064x}")
    }
}
