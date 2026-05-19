use axum::{body::Body, extract::Request};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Span;

pub fn make_http_trace_layer() -> TraceLayer<
    tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>,
    impl Fn(&Request<Body>) -> Span + Clone,
    (),
    DefaultOnResponse,
> {
    TraceLayer::new_for_http()
        .make_span_with(|request: &Request<Body>| {
            tracing::info_span!(
                "request",
                method = %request.method(),
                path = %request.uri().path(),
                version = ?request.version(),
            )
        })
        .on_request(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn http_trace_span_records_path_without_uri_query() {
        let source = include_str!("trace.rs");

        let implementation = source
            .split("#[cfg(test)]")
            .next()
            .expect("trace implementation precedes tests");

        assert!(implementation.contains("request.uri().path()"));
        assert!(!implementation.contains("uri ="));
    }
}
