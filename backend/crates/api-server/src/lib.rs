pub mod routes;

pub use routes::{build_router, ApiState, HealthResponse};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    #[test]
    fn exposes_api_router_for_reuse() {
        let _router_builder: fn(Arc<crate::routes::ApiState>) -> axum::Router =
            crate::routes::build_router;
    }
}
