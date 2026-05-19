use argon2::{
    password_hash::{PasswordHash, PasswordVerifier},
    Argon2,
};
use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};
use chrono::{Duration, Utc};
use coin_listener_core::{AppError, AppResult};
use coin_listener_storage::repositories;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TokenSettings {
    pub secret: String,
    pub ttl: Duration,
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthClaims {
    pub sub: String,
    pub tenant_id: String,
    pub email: String,
    pub iat: i64,
    pub exp: i64,
}

impl AuthClaims {
    pub fn subject_uuid(&self) -> AppResult<Uuid> {
        Uuid::parse_str(&self.sub).map_err(|_| AppError::Unauthorized)
    }

    pub fn tenant_uuid(&self) -> AppResult<Uuid> {
        Uuid::parse_str(&self.tenant_id).map_err(|_| AppError::Unauthorized)
    }
}

const MIN_TOKEN_SECRET_BYTES: usize = 32;

pub fn token_settings(secret: String, ttl_seconds: i64) -> AppResult<TokenSettings> {
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        return Err(AppError::Config(
            "AUTH_TOKEN_SECRET is required".to_string(),
        ));
    }
    if secret.len() < MIN_TOKEN_SECRET_BYTES {
        return Err(AppError::Config(
            "AUTH_TOKEN_SECRET must be at least 32 bytes".to_string(),
        ));
    }
    if ttl_seconds <= 0 {
        return Err(AppError::Config(
            "AUTH_TOKEN_TTL_SECONDS must be positive".to_string(),
        ));
    }

    Ok(TokenSettings {
        secret,
        ttl: Duration::seconds(ttl_seconds),
    })
}

pub fn verify_password(password: &str, password_hash: &str) -> AppResult<bool> {
    let parsed_hash = match PasswordHash::new(password_hash) {
        Ok(hash) => hash,
        Err(_) => return Ok(false),
    };

    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

pub fn issue_token(
    settings: &TokenSettings,
    user_id: Uuid,
    tenant_id: Uuid,
    email: &str,
) -> AppResult<String> {
    let issued_at = Utc::now();
    let expires_at = issued_at + settings.ttl;
    let claims = AuthClaims {
        sub: user_id.to_string(),
        tenant_id: tenant_id.to_string(),
        email: email.to_string(),
        iat: issued_at.timestamp(),
        exp: expires_at.timestamp(),
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(settings.secret.as_bytes()),
    )
    .map_err(|error| AppError::Config(error.to_string()))
}

pub fn validate_token(settings: &TokenSettings, token: &str) -> AppResult<AuthClaims> {
    let mut validation = Validation::default();
    validation.leeway = 0;

    decode::<AuthClaims>(
        token,
        &DecodingKey::from_secret(settings.secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|_| AppError::Unauthorized)
}

pub fn bearer_token(headers: &axum::http::HeaderMap) -> AppResult<&str> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.trim().is_empty())
        .ok_or(AppError::Unauthorized)
}

pub async fn require_auth(
    State(state): State<Arc<crate::ApiState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, crate::ApiError> {
    let token = bearer_token(request.headers())?;
    let claims = validate_token(&state.auth, token)?;
    let user_id = claims.subject_uuid()?;
    let tenant_id = claims.tenant_uuid()?;
    repositories::active_tenant_membership(&state.postgres, user_id, tenant_id).await?;

    request.extensions_mut().insert(AuthContext {
        user_id,
        tenant_id,
        email: claims.email,
    });

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    #[test]
    fn verifies_argon2id_password_hash() {
        let hash = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXJhbmRvbXNhbHQ$laqOUbdkJho4NACYmDwyLQdS/qq83rIuReZa+IyST2I";

        assert!(super::verify_password("admin", hash).expect("password verifies"));
        assert!(!super::verify_password("wrong", hash).expect("password rejects"));
    }

    #[test]
    fn rejects_plaintext_password_hashes() {
        assert!(!super::verify_password("admin", "admin").expect("plaintext rejected"));
    }

    #[test]
    fn rejects_empty_token_secret() {
        let error = super::token_settings("   ".to_string(), 3600)
            .expect_err("empty token secret is rejected");

        assert!(matches!(error, coin_listener_core::AppError::Config(_)));
    }

    #[test]
    fn rejects_short_token_secret() {
        let error = super::token_settings("short-secret".to_string(), 3600)
            .expect_err("short token secret is rejected");

        assert!(matches!(error, coin_listener_core::AppError::Config(_)));
    }

    #[test]
    fn rejects_non_positive_token_ttl() {
        let error = super::token_settings("test-secret-with-enough-entropy-32".to_string(), 0)
            .expect_err("non-positive token ttl is rejected");

        assert!(matches!(error, coin_listener_core::AppError::Config(_)));
    }

    #[test]
    fn invalid_claim_uuids_are_unauthorized() {
        let claims = super::AuthClaims {
            sub: "not-a-uuid".to_string(),
            tenant_id: "not-a-uuid".to_string(),
            email: "admin@example.com".to_string(),
            iat: Utc::now().timestamp(),
            exp: (Utc::now() + Duration::seconds(3600)).timestamp(),
        };

        assert!(matches!(
            claims.subject_uuid(),
            Err(coin_listener_core::AppError::Unauthorized)
        ));
        assert!(matches!(
            claims.tenant_uuid(),
            Err(coin_listener_core::AppError::Unauthorized)
        ));
    }

    #[test]
    fn token_round_trips_claims() {
        let user_id = Uuid::from_u128(7);
        let tenant_id = Uuid::from_u128(9);
        let settings = super::TokenSettings {
            secret: "test-secret-with-enough-entropy".to_string(),
            ttl: Duration::seconds(3600),
        };

        let token = super::issue_token(&settings, user_id, tenant_id, "admin@example.com")
            .expect("token issued");
        let claims = super::validate_token(&settings, &token).expect("token validates");

        assert_eq!(claims.subject_uuid().unwrap(), user_id);
        assert_eq!(claims.tenant_uuid().unwrap(), tenant_id);
        assert_eq!(claims.email, "admin@example.com");
        assert!(claims.exp > Utc::now().timestamp());
    }

    #[test]
    fn rejects_tampered_tokens() {
        let settings = super::TokenSettings {
            secret: "test-secret-with-enough-entropy".to_string(),
            ttl: Duration::seconds(3600),
        };
        let token = super::issue_token(
            &settings,
            Uuid::from_u128(7),
            Uuid::from_u128(9),
            "admin@example.com",
        )
        .expect("token issued");
        let tampered = format!("{}x", token);

        assert!(super::validate_token(&settings, &tampered).is_err());
    }

    #[test]
    fn rejects_expired_tokens() {
        let settings = super::TokenSettings {
            secret: "test-secret-with-enough-entropy".to_string(),
            ttl: Duration::seconds(-1),
        };
        let token = super::issue_token(
            &settings,
            Uuid::from_u128(7),
            Uuid::from_u128(9),
            "admin@example.com",
        )
        .expect("token issued");

        assert!(super::validate_token(&settings, &token).is_err());
    }
}
