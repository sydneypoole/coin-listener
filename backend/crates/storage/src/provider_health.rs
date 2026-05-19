use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

pub const PROVIDER_CIRCUIT_FAILURE_THRESHOLD: i32 = 3;
pub const PROVIDER_CIRCUIT_COOLDOWN_SECONDS: i64 = 300;
pub const PROVIDER_LAST_ERROR_MAX_CHARS: usize = 500;

pub fn provider_qps_key(provider_id: Uuid, epoch_second: i64) -> String {
    format!("provider:qps:{provider_id}:{epoch_second}")
}

pub fn provider_disabled_until(failure_count: i32, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    (failure_count >= PROVIDER_CIRCUIT_FAILURE_THRESHOLD)
        .then(|| now + Duration::seconds(PROVIDER_CIRCUIT_COOLDOWN_SECONDS))
}

pub fn is_provider_circuit_open(disabled_until: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    disabled_until.is_some_and(|value| value > now)
}

pub fn sanitize_provider_error(error: &str) -> String {
    let mut sanitized = error
        .replace("token=", "token=<redacted>")
        .replace("api_key=", "api_key=<redacted>")
        .replace("key=", "key=<redacted>");

    sanitized = redact_query_value(&sanitized, "token=<redacted>");
    sanitized = redact_query_value(&sanitized, "api_key=<redacted>");
    sanitized = redact_query_value(&sanitized, "key=<redacted>");

    sanitized.chars().take(PROVIDER_LAST_ERROR_MAX_CHARS).collect()
}

fn redact_query_value(input: &str, marker: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(index) = remaining.find(marker) {
        output.push_str(&remaining[..index + marker.len()]);
        let after_marker = &remaining[index + marker.len()..];
        let next_separator = after_marker
            .find('&')
            .or_else(|| after_marker.find(' '))
            .unwrap_or(after_marker.len());
        remaining = &after_marker[next_separator..];
    }

    output.push_str(remaining);
    output
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::provider_health::{
        is_provider_circuit_open, provider_disabled_until, provider_qps_key,
        sanitize_provider_error, PROVIDER_CIRCUIT_COOLDOWN_SECONDS,
        PROVIDER_CIRCUIT_FAILURE_THRESHOLD, PROVIDER_LAST_ERROR_MAX_CHARS,
    };

    #[test]
    fn provider_health_migration_defines_table_and_indexes() {
        let migration = include_str!("../migrations/0011_provider_health.sql");

        assert!(migration.contains("CREATE TABLE IF NOT EXISTS provider_health"));
        assert!(migration.contains("provider_id UUID PRIMARY KEY REFERENCES providers(id) ON DELETE CASCADE"));
        assert!(migration.contains("disabled_until TIMESTAMPTZ"));
        assert!(migration.contains("idx_provider_health_disabled_until"));
        assert!(migration.contains("idx_provider_health_last_failure"));
    }

    #[test]
    fn provider_qps_key_uses_provider_id_and_epoch_second() {
        let provider_id = uuid::Uuid::from_u128(42);

        assert_eq!(
            provider_qps_key(provider_id, 1_779_123_600),
            "provider:qps:00000000-0000-0000-0000-00000000002a:1779123600"
        );
    }

    #[test]
    fn provider_disabled_until_uses_five_minute_cooldown() {
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap();

        assert_eq!(PROVIDER_CIRCUIT_FAILURE_THRESHOLD, 3);
        assert_eq!(PROVIDER_CIRCUIT_COOLDOWN_SECONDS, 300);
        assert_eq!(
            provider_disabled_until(PROVIDER_CIRCUIT_FAILURE_THRESHOLD, now),
            Some(Utc.with_ymd_and_hms(2026, 5, 19, 10, 5, 0).unwrap())
        );
        assert_eq!(provider_disabled_until(PROVIDER_CIRCUIT_FAILURE_THRESHOLD - 1, now), None);
    }

    #[test]
    fn circuit_is_open_only_while_disabled_until_is_future() {
        let now = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 0).unwrap();
        let past = Utc.with_ymd_and_hms(2026, 5, 19, 9, 59, 59).unwrap();
        let future = Utc.with_ymd_and_hms(2026, 5, 19, 10, 0, 1).unwrap();

        assert!(!is_provider_circuit_open(None, now));
        assert!(!is_provider_circuit_open(Some(past), now));
        assert!(is_provider_circuit_open(Some(future), now));
    }

    #[test]
    fn provider_error_sanitizer_redacts_query_secrets_and_truncates() {
        let secret = format!(
            "provider request failed: https://example.invalid/rpc?token=abc&api_key=def&key=ghi&safe=ok {}",
            "x".repeat(PROVIDER_LAST_ERROR_MAX_CHARS + 20)
        );

        let sanitized = sanitize_provider_error(&secret);

        assert!(!sanitized.contains("abc"));
        assert!(!sanitized.contains("def"));
        assert!(!sanitized.contains("ghi"));
        assert!(sanitized.contains("token=<redacted>"));
        assert!(sanitized.contains("api_key=<redacted>"));
        assert!(sanitized.contains("key=<redacted>"));
        assert!(sanitized.len() <= PROVIDER_LAST_ERROR_MAX_CHARS);
    }
}
