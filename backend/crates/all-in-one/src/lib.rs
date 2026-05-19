use axum::{
    body::Body,
    http::{header, HeaderMap, Method, Request, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use std::{convert::Infallible, env, path::PathBuf, sync::Arc};
use tokio::task::JoinError;
use tower::util::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

pub const FRONTEND_DIST_ENV: &str = "COIN_LISTENER_FRONTEND_DIST";
pub const DEFAULT_FRONTEND_DIST: &str = "./frontend/dist";
pub const ALL_IN_ONE_SERVICE_NAMES: [&str; 4] = ["api-server", "scheduler", "worker", "notifier"];

#[derive(Clone)]
struct FrontendAssets {
    dist: PathBuf,
    index: PathBuf,
}

pub fn frontend_dist_path(value: Option<String>) -> PathBuf {
    value
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_FRONTEND_DIST))
}

pub fn frontend_dist_path_from_env() -> PathBuf {
    frontend_dist_path(env::var(FRONTEND_DIST_ENV).ok())
}

pub fn build_all_in_one_router(api_router: Router, frontend_dist: PathBuf) -> Router {
    let assets = Arc::new(FrontendAssets {
        index: frontend_dist.join("index.html"),
        dist: frontend_dist,
    });

    api_router.fallback(move |request| {
        let assets = Arc::clone(&assets);
        async move { frontend_fallback(request, assets).await }
    })
}

async fn frontend_fallback(request: Request<Body>, assets: Arc<FrontendAssets>) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    if is_api_path(&path) || (method != Method::GET && method != Method::HEAD) {
        return StatusCode::NOT_FOUND.into_response();
    }

    if is_spa_navigation(&path, request.headers()) {
        return serve_index(request, assets.index.clone()).await;
    }

    serve_static(request, assets.dist.clone()).await
}

fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn is_spa_navigation(path: &str, headers: &HeaderMap) -> bool {
    !is_asset_path(path) && accepts_html(headers)
}

fn is_asset_path(path: &str) -> bool {
    path == "/assets"
        || path.starts_with("/assets/")
        || path
            .rsplit('/')
            .next()
            .is_some_and(|segment| segment.contains('.'))
}

fn accepts_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|part| {
                matches!(
                    part.split(';').next().map(str::trim),
                    Some("text/html" | "application/xhtml+xml")
                )
            })
        })
}

async fn serve_static(request: Request<Body>, dist: PathBuf) -> Response {
    match ServeDir::new(dist).oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(error) => static_asset_error(error),
    }
}

async fn serve_index(request: Request<Body>, index: PathBuf) -> Response {
    match ServeFile::new(index).oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(error) => static_asset_error(error),
    }
}

fn static_asset_error(error: Infallible) -> Response {
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

    fn repository_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap()
            .to_path_buf()
    }

    fn compose_service_block(compose: &str, service: &str) -> String {
        let marker = format!("  {service}:");
        let mut block = Vec::new();
        let mut in_service = false;

        for line in compose.lines() {
            if line == marker {
                in_service = true;
                block.push(line);
                continue;
            }

            if in_service
                && line.starts_with("  ")
                && !line.starts_with("    ")
                && !line.trim().is_empty()
            {
                break;
            }

            if in_service {
                block.push(line);
            }
        }

        assert!(in_service, "missing compose service {service}");
        block.join("\n")
    }

    #[test]
    fn dockerfile_builds_binary_and_copies_frontend_dist() {
        let dockerfile =
            fs::read_to_string(repository_root().join("docker/all-in-one.Dockerfile")).unwrap();

        assert!(dockerfile.contains("npm ci"));
        assert!(dockerfile.contains("npm run build"));
        assert!(dockerfile.contains("cargo build --release --workspace --bin all-in-one"));
        assert!(dockerfile.contains("/usr/local/bin/all-in-one"));
        assert!(dockerfile.contains("/usr/local/share/coin-listener/frontend"));
        assert!(dockerfile.contains("COIN_LISTENER_FRONTEND_DIST"));
    }

    #[test]
    fn compose_exposes_all_in_one_profile_without_removing_multi_process_services() {
        let compose = fs::read_to_string(repository_root().join("docker-compose.yml")).unwrap();
        let all_in_one = compose_service_block(&compose, "all-in-one");

        assert!(all_in_one.contains("profiles:"));
        assert!(all_in_one.contains("all-in-one"));
        assert!(all_in_one.contains("docker/all-in-one.Dockerfile"));
        assert!(compose.contains("api-server:"));
        assert!(compose.contains("scheduler:"));
        assert!(compose.contains("worker:"));
        assert!(compose.contains("notifier:"));
    }

    #[test]
    fn compose_default_app_services_are_separated_from_all_in_one_profile() {
        let compose = fs::read_to_string(repository_root().join("docker-compose.yml")).unwrap();

        for service in ["api-server", "scheduler", "worker", "notifier"] {
            let block = compose_service_block(&compose, service);
            assert!(block.contains("profiles:"), "{service} missing profile");
            assert!(
                block.contains("multi-process"),
                "{service} missing multi-process profile"
            );
            assert!(
                !block.contains("all-in-one"),
                "{service} should not be in all-in-one profile"
            );
        }
    }

    #[test]
    fn env_example_documents_frontend_dist_path() {
        let env_example = fs::read_to_string(repository_root().join(".env.example")).unwrap();

        assert!(env_example.contains("COIN_LISTENER_FRONTEND_DIST=./frontend/dist"));
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
                    .header("accept", "text/html")
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

    #[tokio::test]
    async fn unknown_api_routes_do_not_return_frontend_index() {
        let dist = test_dist("api-not-found");
        let api = Router::new().route("/health", get(|| async { "api-health" }));
        let app = crate::build_all_in_one_router(api, dist.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/missing")
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        assert_ne!(&body[..], b"frontend-shell");
        fs::remove_dir_all(dist).unwrap();
    }

    #[tokio::test]
    async fn missing_assets_do_not_return_frontend_index() {
        let dist = test_dist("missing-asset");
        let api = Router::new().route("/health", get(|| async { "api-health" }));
        let app = crate::build_all_in_one_router(api, dist.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/assets/missing.js")
                    .header("accept", "*/*")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        assert_ne!(&body[..], b"frontend-shell");
        fs::remove_dir_all(dist).unwrap();
    }

    #[tokio::test]
    async fn non_get_frontend_routes_do_not_return_frontend_index() {
        let dist = test_dist("non-get");
        let api = Router::new().route("/health", get(|| async { "api-health" }));
        let app = crate::build_all_in_one_router(api, dist.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/events")
                    .header("accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        assert_ne!(&body[..], b"frontend-shell");
        fs::remove_dir_all(dist).unwrap();
    }

    #[tokio::test]
    async fn missing_frontend_dist_does_not_block_api_routes() {
        let dist = std::env::temp_dir().join(format!(
            "coin-listener-all-in-one-missing-dist-{}",
            uuid::Uuid::new_v4()
        ));
        let api = Router::new().route("/health", get(|| async { "api-health" }));
        let app = crate::build_all_in_one_router(api, dist);

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
    }

    #[test]
    fn frontend_dist_path_ignores_empty_override() {
        assert_eq!(
            crate::frontend_dist_path(Some("".to_string())),
            PathBuf::from(crate::DEFAULT_FRONTEND_DIST)
        );
    }

    #[test]
    fn service_task_result_names_failed_service() {
        let error = coin_listener_core::AppError::Config("bad config".to_string());
        let result = crate::service_task_result("worker", Ok(Err(error)));

        assert!(result.unwrap_err().to_string().contains("worker failed"));
    }
}
