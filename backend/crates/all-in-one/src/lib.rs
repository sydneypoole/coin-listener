use axum::{response::Response, routing::get_service, Router};
use std::{convert::Infallible, env, path::PathBuf};
use tokio::task::JoinError;
use tower_http::services::{ServeDir, ServeFile};

pub const FRONTEND_DIST_ENV: &str = "COIN_LISTENER_FRONTEND_DIST";
pub const DEFAULT_FRONTEND_DIST: &str = "./frontend/dist";
pub const ALL_IN_ONE_SERVICE_NAMES: [&str; 4] = ["api-server", "scheduler", "worker", "notifier"];

pub fn frontend_dist_path(value: Option<String>) -> PathBuf {
    value
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_FRONTEND_DIST))
}

pub fn frontend_dist_path_from_env() -> PathBuf {
    frontend_dist_path(env::var(FRONTEND_DIST_ENV).ok())
}

pub fn build_all_in_one_router(api_router: Router, frontend_dist: PathBuf) -> Router {
    let index = frontend_dist.join("index.html");
    let static_service = ServeDir::new(frontend_dist).fallback(ServeFile::new(index));

    api_router.fallback_service(get_service(static_service).handle_error(static_asset_error))
}

async fn static_asset_error(error: Infallible) -> Response {
    match error {}
}

pub fn service_task_result(
    service: &'static str,
    result: Result<coin_listener_core::AppResult<()>, JoinError>,
) -> anyhow::Result<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(anyhow::anyhow!("{service} failed: {error}")),
        Err(error) => Err(anyhow::anyhow!("{service} task failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use std::{fs, path::PathBuf};
    use tower::ServiceExt;

    fn test_dist(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "coin-listener-all-in-one-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("index.html"), "frontend-shell").unwrap();
        path
    }

    #[test]
    fn frontend_dist_path_uses_default_and_env_override() {
        assert_eq!(
            crate::frontend_dist_path(None),
            PathBuf::from(crate::DEFAULT_FRONTEND_DIST)
        );
        assert_eq!(
            crate::frontend_dist_path(Some("/opt/coin-listener/dist".to_string())),
            PathBuf::from("/opt/coin-listener/dist")
        );
    }

    #[tokio::test]
    async fn api_routes_take_precedence_over_static_fallback() {
        let dist = test_dist("api-priority");
        let api = Router::new().route("/health", get(|| async { "api-health" }));
        let app = crate::build_all_in_one_router(api, dist.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        assert_eq!(&body[..], b"api-health");
        fs::remove_dir_all(dist).unwrap();
    }

    #[tokio::test]
    async fn non_api_routes_fall_back_to_frontend_index() {
        let dist = test_dist("spa-fallback");
        let api = Router::new().route("/health", get(|| async { "api-health" }));
        let app = crate::build_all_in_one_router(api, dist.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        assert_eq!(&body[..], b"frontend-shell");
        fs::remove_dir_all(dist).unwrap();
    }

    #[test]
    fn service_task_result_names_failed_service() {
        let error = coin_listener_core::AppError::Config("bad config".to_string());
        let result = crate::service_task_result("worker", Ok(Err(error)));

        assert!(result.unwrap_err().to_string().contains("worker failed"));
    }
}
