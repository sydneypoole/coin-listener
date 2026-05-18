FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --workspace

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/api-server /usr/local/bin/api-server
COPY --from=builder /app/target/release/scheduler /usr/local/bin/scheduler
COPY --from=builder /app/target/release/worker /usr/local/bin/worker
COPY --from=builder /app/target/release/notifier /usr/local/bin/notifier
ENTRYPOINT []
