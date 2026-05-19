FROM node:24-bookworm AS frontend-builder
WORKDIR /app/frontend
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1-bookworm AS backend-builder
WORKDIR /app/backend
COPY backend/ ./
RUN cargo build --release --workspace --bin all-in-one

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=backend-builder /app/backend/target/release/all-in-one /usr/local/bin/all-in-one
COPY --from=frontend-builder /app/frontend/dist /usr/local/share/coin-listener/frontend
ENV COIN_LISTENER_FRONTEND_DIST=/usr/local/share/coin-listener/frontend
ENTRYPOINT ["all-in-one"]
