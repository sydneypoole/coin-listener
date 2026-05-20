#!/usr/bin/env sh
set -eu

API_SERVER_HOST=127.0.0.1
API_SERVER_PORT=18080
export API_SERVER_HOST API_SERVER_PORT

all-in-one &
backend_pid="$!"

nginx -g 'daemon off;' &
nginx_pid="$!"

term() {
  kill -TERM "$backend_pid" "$nginx_pid" 2>/dev/null || true
}
trap term INT TERM

while :; do
  if ! kill -0 "$backend_pid" 2>/dev/null; then
    wait "$backend_pid"
    status="$?"
    kill -TERM "$nginx_pid" 2>/dev/null || true
    wait "$nginx_pid" 2>/dev/null || true
    exit "$status"
  fi

  if ! kill -0 "$nginx_pid" 2>/dev/null; then
    wait "$nginx_pid"
    status="$?"
    kill -TERM "$backend_pid" 2>/dev/null || true
    wait "$backend_pid" 2>/dev/null || true
    exit "$status"
  fi

  sleep 1
done
