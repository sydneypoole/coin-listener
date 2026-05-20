# Coin Listener Deployment

This guide covers deploying Coin Listener as the all-in-one runtime: one application process that serves the API, serves the built React frontend, and runs scheduler, worker, notifier, realtime notification, and heartbeat loops. PostgreSQL and Redis remain external services.

## Deployment options

| Option | Use case | Entry point |
|---|---|---|
| Docker Compose all-in-one | Fastest server deployment from local source | `docker compose --profile all-in-one up -d --build` |
| GHCR Docker Compose | Run the published GitHub image directly | `COIN_LISTENER_IMAGE=ghcr.io/<owner>/<repo>-all-in-one:latest docker compose -f docker-compose.ghcr.yml up -d` |
| GitHub Actions artifacts | Download prebuilt frontend/backend/Docker outputs | `.github/workflows/package.yml` |
| GHCR image | Pull image built from `main`, branches, tags, or SHA | `ghcr.io/<owner>/<repo>-all-in-one:<tag>` |
| Multi-process Compose | Run API, scheduler, worker, notifier separately | `docker compose --profile multi-process up -d --build` |

## Prerequisites

- Docker and Docker Compose for container deployment.
- PostgreSQL 16 and Redis 7 when not using the bundled Compose services.
- A long random `AUTH_TOKEN_SECRET` before exposing the service.
- Network access from the app to PostgreSQL and Redis.

## Environment file

Copy the example and edit secrets before starting services:

```bash
cp .env.example .env
```

Important variables:

| Variable | Required | Default/example | Notes |
|---|---:|---|---|
| `DATABASE_URL` | Yes | `postgres://coin_listener:coin_listener_password@postgres:5432/coin_listener` | PostgreSQL connection used by API, migrations, scheduler, worker, notifier, realtime listener. |
| `REDIS_URL` | Yes | `redis://redis:6379` | Redis queue and status backend. |
| `API_SERVER_HOST` | Yes | `0.0.0.0` | Bind host inside container. |
| `API_SERVER_PORT` | Yes | `8080` | HTTP port. Compose maps this to host `8080`. |
| `AUTH_TOKEN_SECRET` | Yes | `change-me-to-a-long-random-secret` | Replace with a production secret. |
| `AUTH_TOKEN_TTL_SECONDS` | No | `43200` | Login token TTL. |
| `COIN_LISTENER_FRONTEND_DIST` | No for Docker images | `/usr/local/share/coin-listener/frontend` | The all-in-one Docker image already sets this to its bundled frontend assets. Only set it for manual binary runs. |
| `ENABLE_DEV_ROUTES` | No | `false` | Keep disabled in production. |
| `RUST_LOG` | No | `info` | Runtime log level. |
| `SCAN_QUEUE_KEY` | No | `scan:address:queue` | Redis scan queue key. |
| `NOTIFY_QUEUE_KEY` | No | `notify:event:queue` | Notification queue key. |

For Docker Compose all-in-one deployment, keep `POSTGRES_HOST=postgres`, `DATABASE_URL=...@postgres:5432/...`, and `REDIS_URL=redis://redis:6379` unless using external services.

## Deploy with Docker Compose all-in-one

1. Create and edit `.env`:

```bash
cp .env.example .env
```

2. Start PostgreSQL, Redis, and the all-in-one app:

```bash
docker compose --profile all-in-one up -d --build
```

3. Check container status:

```bash
docker compose ps
```

4. Check health:

```bash
curl http://localhost:8080/health
```

5. Open the UI:

```text
http://localhost:8080
```

The application runs migrations at startup. The baseline seed creates `admin@example.com`; the migration updates the legacy default password hash for password `admin`. Change this account before production use.

## Deploy with the GHCR Docker Compose file

Use this when you want the server to pull and run the image published by GitHub Actions instead of building locally. The image already contains the compiled frontend and sets the frontend asset path internally, so you do not need to configure `COIN_LISTENER_FRONTEND_DIST` for Docker deployment.

1. Optionally create `.env` to override defaults and set production secrets:

```bash
cp .env.example .env
```

At minimum, set `AUTH_TOKEN_SECRET` before production use.

2. Set the image name and start the stack:

```bash
COIN_LISTENER_IMAGE=ghcr.io/<owner>/<repo>-all-in-one:latest docker compose -f docker-compose.ghcr.yml up -d
```

Use another published tag when needed:

```bash
COIN_LISTENER_IMAGE=ghcr.io/<owner>/<repo>-all-in-one:sha-<12-char-sha> docker compose -f docker-compose.ghcr.yml up -d
```

3. Pull a newer image and restart:

```bash
COIN_LISTENER_IMAGE=ghcr.io/<owner>/<repo>-all-in-one:latest docker compose -f docker-compose.ghcr.yml pull
COIN_LISTENER_IMAGE=ghcr.io/<owner>/<repo>-all-in-one:latest docker compose -f docker-compose.ghcr.yml up -d
```

4. Stop the stack:

```bash
docker compose -f docker-compose.ghcr.yml down
```

`docker-compose.ghcr.yml` still starts PostgreSQL and Redis locally. If you use external PostgreSQL or Redis, set `DATABASE_URL` and `REDIS_URL` in `.env` or in the shell before running `docker compose`. Do not set `COIN_LISTENER_FRONTEND_DIST` for the GHCR image unless you intentionally override the bundled frontend path.

## Deploy with the GHCR image manually

The packaging workflow pushes GHCR images only on `push` events. Tags include:

| Tag | Created when |
|---|---|
| `sha-<12-char-sha>` | Every push |
| `<branch-name>` | Branch push, sanitized for Docker tag rules |
| `latest` | Default branch push |
| `<git-tag>` | Git tag push |

Pull and run an image:

```bash
docker pull ghcr.io/<owner>/<repo>-all-in-one:latest
```

Example container run using external PostgreSQL and Redis:

```bash
docker run --rm -p 8080:8080 \
  -e DATABASE_URL='postgres://coin_listener:coin_listener_password@db-host:5432/coin_listener' \
  -e REDIS_URL='redis://redis-host:6379' \
  -e API_SERVER_HOST='0.0.0.0' \
  -e API_SERVER_PORT='8080' \
  -e AUTH_TOKEN_SECRET='<long-random-secret>' \
  ghcr.io/<owner>/<repo>-all-in-one:latest
```

## GitHub Actions packaging

Workflow file: `.github/workflows/package.yml`.

Triggers:

- `push` to any branch and `v*` tags.
- `pull_request`.
- `workflow_dispatch`.

Artifacts:

| Artifact | Contents |
|---|---|
| `coin-listener-frontend-dist` | Zip of `frontend/dist`. |
| `coin-listener-all-in-one-linux-amd64` | `all-in-one` release executable packaged as `.tar.gz`. |
| `coin-listener-all-in-one-docker-image` | `docker save` archive of the all-in-one image, compressed with gzip. |

The `docker-push` job is push-only and is the only job with `packages: write` permission.

## Using downloaded artifacts

### Frontend zip

Unzip the `coin-listener-frontend-dist` artifact and serve the files with any static server, or place them where `COIN_LISTENER_FRONTEND_DIST` points for a local all-in-one binary run.

### Backend executable

Extract the backend artifact:

```bash
tar -xzf coin-listener-all-in-one-linux-amd64-<sha>.tar.gz
chmod +x all-in-one
```

Run it with environment variables:

```bash
DATABASE_URL='postgres://coin_listener:coin_listener_password@localhost:5432/coin_listener' \
REDIS_URL='redis://localhost:6379' \
API_SERVER_HOST='0.0.0.0' \
API_SERVER_PORT='8080' \
AUTH_TOKEN_SECRET='<long-random-secret>' \
COIN_LISTENER_FRONTEND_DIST='./frontend/dist' \
./all-in-one
```

### Docker image archive

Load the downloaded Docker archive:

```bash
gzip -dc coin-listener-all-in-one-docker-<sha>.tar.gz | docker load
```

Then run the loaded image tag shown by `docker load` with the environment variables above.

## Local build commands

Frontend:

```bash
npm ci --prefix frontend
npm run build --prefix frontend
```

Backend executable:

```bash
cargo build --locked --release --manifest-path backend/Cargo.toml --bin all-in-one
```

Docker image:

```bash
docker build -f docker/all-in-one.Dockerfile -t coin-listener-all-in-one:local .
```

## Operational checks

| Check | Command |
|---|---|
| Service health | `curl http://localhost:8080/health` |
| Auth endpoint reachable | `curl -i http://localhost:8080/api/auth/login` should not return a network error. |
| System status after login | UI page `系统状态`, or `GET /api/system/status` with bearer token. |
| Logs for local-build Compose | `docker compose logs -f all-in-one` |
| Logs for GHCR Compose | `docker compose -f docker-compose.ghcr.yml logs -f all-in-one` |
| Stop local-build Compose deployment | `docker compose --profile all-in-one down` |
| Stop GHCR Compose deployment | `docker compose -f docker-compose.ghcr.yml down` |

## Production notes

- Replace `AUTH_TOKEN_SECRET` and database passwords before exposing the service.
- Do not enable `ENABLE_DEV_ROUTES` in production.
- Keep PostgreSQL and Redis storage persistent and backed up.
- Put TLS and public routing in front of port `8080` with your reverse proxy or load balancer.
- Do not run all-in-one and multi-process scheduler/worker/notifier services against the same queues unless duplicate processing capacity is intentional.
