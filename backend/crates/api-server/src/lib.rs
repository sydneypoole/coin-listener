pub mod auth;
pub mod realtime;
mod routes;
mod trace;

pub use routes::{build_router, ApiError, ApiState, HealthResponse};
pub use trace::make_http_trace_layer;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    #[test]
    fn exposes_api_router_for_reuse() {
        let _router_builder: fn(Arc<crate::ApiState>) -> axum::Router = crate::build_router;
    }
}
