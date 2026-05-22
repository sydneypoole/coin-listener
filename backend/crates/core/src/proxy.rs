use crate::{AppError, AppResult};

pub const TELEGRAM_PROXY_SOURCE_BOT: &str = "bot";
pub const TELEGRAM_PROXY_SOURCE_GLOBAL: &str = "global";
pub const TELEGRAM_PROXY_SOURCE_DIRECT: &str = "direct";

pub fn normalize_proxy_url(value: Option<&str>) -> AppResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if !valid_proxy_url(value) {
        return Err(AppError::Validation(
            "proxy_url must use http, https, or socks5 with a host".to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

pub fn mask_proxy_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let path = &rest[authority_end..];
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return url.to_string();
    };
    if host.is_empty() {
        return url.to_string();
    }
    let username = userinfo.split(':').next().unwrap_or("");
    if username.is_empty() {
        return format!("{scheme}://***@{host}{path}");
    }
    format!("{scheme}://{username}:***@{host}{path}")
}

pub fn telegram_proxy_source(
    bot_proxy_url: Option<&str>,
    global_proxy_url: Option<&str>,
) -> &'static str {
    if bot_proxy_url.is_some_and(|value| !value.trim().is_empty()) {
        TELEGRAM_PROXY_SOURCE_BOT
    } else if global_proxy_url.is_some_and(|value| !value.trim().is_empty()) {
        TELEGRAM_PROXY_SOURCE_GLOBAL
    } else {
        TELEGRAM_PROXY_SOURCE_DIRECT
    }
}

fn valid_proxy_url(value: &str) -> bool {
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https" | "socks5") {
        return false;
    }
    if rest.is_empty() || rest.starts_with('/') {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or(rest);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    !host_port.is_empty() && !host_port.starts_with(':')
}

#[cfg(test)]
mod tests {
    use super::{mask_proxy_url, normalize_proxy_url, telegram_proxy_source};

    #[test]
    fn normalize_proxy_url_accepts_supported_schemes() {
        assert_eq!(
            normalize_proxy_url(Some(" http://user:pass@127.0.0.1:7890 ")).unwrap(),
            Some("http://user:pass@127.0.0.1:7890".to_string())
        );
        assert_eq!(
            normalize_proxy_url(Some("https://proxy.example.com:443")).unwrap(),
            Some("https://proxy.example.com:443".to_string())
        );
        assert_eq!(
            normalize_proxy_url(Some("socks5://127.0.0.1:1080")).unwrap(),
            Some("socks5://127.0.0.1:1080".to_string())
        );
        assert_eq!(normalize_proxy_url(Some("   ")).unwrap(), None);
        assert_eq!(normalize_proxy_url(None).unwrap(), None);
    }

    #[test]
    fn normalize_proxy_url_rejects_unsupported_or_incomplete_urls() {
        for value in [
            "ftp://proxy.example.com:21",
            "http:///missing-host",
            "https://",
            "socks5:// user:pass@host:1080",
            "proxy.example.com:7890",
        ] {
            let error = normalize_proxy_url(Some(value)).unwrap_err().to_string();
            assert!(error.contains("proxy_url must use http, https, or socks5"));
        }
    }

    #[test]
    fn mask_proxy_url_redacts_credentials() {
        assert_eq!(
            mask_proxy_url("http://alice:secret@proxy.example.com:7890"),
            "http://alice:***@proxy.example.com:7890"
        );
        assert_eq!(
            mask_proxy_url("http://:secret@proxy.example.com:7890"),
            "http://***@proxy.example.com:7890"
        );
        assert_eq!(
            mask_proxy_url("socks5://proxy.example.com:1080"),
            "socks5://proxy.example.com:1080"
        );
    }

    #[test]
    fn telegram_proxy_source_prefers_bot_then_global_then_direct() {
        assert_eq!(
            telegram_proxy_source(Some("http://bot:7890"), Some("http://global:7890")),
            "bot"
        );
        assert_eq!(
            telegram_proxy_source(None, Some("http://global:7890")),
            "global"
        );
        assert_eq!(telegram_proxy_source(None, None), "direct");
    }
}
