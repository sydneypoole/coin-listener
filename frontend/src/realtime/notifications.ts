import { ApiRequestError } from '../api/client';
import type { InAppNotification, LoginResponse, RealtimeServerMessage } from '../api/types';

const REALTIME_PATH = '/api/realtime/notifications';
const MAX_RECONNECT_DELAY_MS = 30_000;

type RealtimeNotificationMessage = Extract<RealtimeServerMessage, { type: 'in_app_notification.created' }>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isNotification(value: unknown): value is InAppNotification {
  return (
    isRecord(value) &&
    isString(value.id) &&
    isString(value.tenant_id) &&
    isString(value.event_id) &&
    isString(value.title) &&
    isString(value.body) &&
    isString(value.created_at)
  );
}

export function realtimeWebSocketUrl(apiBaseUrl: string, token: string): string {
  const base = apiBaseUrl || window.location.origin;
  const url = new URL(REALTIME_PATH, base);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  url.searchParams.set('token', token);
  return url.toString();
}

export function parseRealtimeMessage(raw: string): RealtimeServerMessage | null {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!isRecord(parsed) || !isString(parsed.type)) return null;

    if (parsed.type === 'in_app_notification.created' && isNotification(parsed.payload)) {
      return parsed as RealtimeNotificationMessage;
    }

    if (
      parsed.type === 'ping' &&
      isRecord(parsed.payload) &&
      isString(parsed.payload.sent_at)
    ) {
      return parsed as RealtimeServerMessage;
    }

    return null;
  } catch {
    return null;
  }
}

export function reconnectDelayMs(attempt: number): number {
  return Math.min(1000 * 2 ** Math.max(0, attempt), MAX_RECONNECT_DELAY_MS);
}

export function isHttpUnauthorized(error: unknown): boolean {
  return error instanceof ApiRequestError && (error.status === 401 || error.status === 403);
}

export type RealtimeNotificationHandlers = {
  onNotification: (notification: InAppNotification) => void;
  onUnauthorized?: () => void;
};

type AuthProbe = () => Promise<unknown>;

export type RealtimeConnectOptions = {
  apiBaseUrl?: string;
  getGeneration?: () => number;
  generation?: number;
  verifyAuth?: AuthProbe;
};

export function connectRealtimeNotifications(
  session: LoginResponse,
  handlers: RealtimeNotificationHandlers,
  options: RealtimeConnectOptions = {},
): () => void {
  let stopped = false;
  let attempt = 0;
  let opened = false;
  let socket: WebSocket | null = null;
  let reconnectTimer: number | null = null;
  const apiBaseUrl = options.apiBaseUrl ?? import.meta.env.VITE_API_BASE_URL ?? '';
  const initialGeneration = options.generation;

  const isStale = () =>
    initialGeneration !== undefined &&
    options.getGeneration !== undefined &&
    options.getGeneration() !== initialGeneration;

  const cleanupTimer = () => {
    if (reconnectTimer !== null) {
      window.clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
  };

  const scheduleReconnect = () => {
    opened = false;
    const delay = reconnectDelayMs(attempt);
    attempt += 1;
    cleanupTimer();
    reconnectTimer = window.setTimeout(connect, delay);
  };

  const probeUnauthorized = () => {
    if (!options.verifyAuth) return false;
    if (stopped || isStale()) return true;

    options.verifyAuth().then(
      () => {
        if (!stopped && !isStale()) scheduleReconnect();
      },
      error => {
        if (stopped || isStale()) return;
        if (isHttpUnauthorized(error)) {
          handlers.onUnauthorized?.();
          return;
        }
        scheduleReconnect();
      },
    );

    return true;
  };

  const connect = () => {
    if (stopped || isStale()) return;
    socket = new WebSocket(realtimeWebSocketUrl(apiBaseUrl, session.token));

    socket.onopen = () => {
      if (stopped || isStale()) {
        socket?.close();
        return;
      }
      opened = true;
      attempt = 0;
    };

    socket.onmessage = event => {
      if (stopped || isStale() || typeof event.data !== 'string') return;
      const message = parseRealtimeMessage(event.data);
      if (message?.type === 'in_app_notification.created') {
        handlers.onNotification(message.payload);
      }
    };

    socket.onclose = event => {
      if (stopped || isStale()) return;
      if (event.code === 1008) {
        handlers.onUnauthorized?.();
        return;
      }
      if (!opened && probeUnauthorized()) return;
      scheduleReconnect();
    };

    socket.onerror = () => {
      socket?.close();
    };
  };

  connect();

  return () => {
    stopped = true;
    cleanupTimer();
    socket?.close();
    socket = null;
  };
}
