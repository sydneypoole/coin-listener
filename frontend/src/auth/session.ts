import type { LoginResponse } from '../api/types';

const SESSION_STORAGE_KEY = 'coin-listener.session.v1';
let currentSession: LoginResponse | null = null;
let unauthorizedHandler: (() => void) | null = null;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isLoginResponse(value: unknown): value is LoginResponse {
  if (!isRecord(value) || !isString(value.token) || value.token.trim() === '') {
    return false;
  }

  const { user, tenant } = value;
  return (
    isRecord(user) &&
    isString(user.id) &&
    isString(user.email) &&
    isString(user.display_name) &&
    isRecord(tenant) &&
    isString(tenant.id) &&
    isString(tenant.name) &&
    isString(tenant.status)
  );
}

function readStoredSession(): string | null {
  if (typeof window === 'undefined') return null;

  try {
    return window.localStorage.getItem(SESSION_STORAGE_KEY);
  } catch {
    return null;
  }
}

function writeStoredSession(session: LoginResponse): void {
  if (typeof window === 'undefined') return;

  try {
    window.localStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(session));
  } catch {
    // Keep the in-memory session usable when browser storage is unavailable.
  }
}

function removeStoredSession(): void {
  if (typeof window === 'undefined') return;

  try {
    window.localStorage.removeItem(SESSION_STORAGE_KEY);
  } catch {
    // Session clearing should not prevent API error handling.
  }
}

function clearStoredSession(): null {
  currentSession = null;
  removeStoredSession();
  return null;
}

export function loadStoredSession(): LoginResponse | null {
  if (typeof window === 'undefined') return currentSession;

  const raw = readStoredSession();
  if (!raw) {
    currentSession = null;
    return null;
  }

  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!isLoginResponse(parsed)) {
      return clearStoredSession();
    }

    currentSession = parsed;
    return currentSession;
  } catch {
    return clearStoredSession();
  }
}

export function saveSession(session: LoginResponse): void {
  currentSession = session;
  writeStoredSession(session);
}

export function clearSession(): void {
  currentSession = null;
  removeStoredSession();
}

export function getAuthToken(): string | null {
  if (currentSession) return currentSession.token;
  return loadStoredSession()?.token ?? null;
}

export function setUnauthorizedHandler(handler: (() => void) | null): void {
  unauthorizedHandler = handler;
}

export function handleUnauthorized(): void {
  clearSession();
  try {
    unauthorizedHandler?.();
  } catch {
    // Keep API request errors authoritative even if UI session cleanup fails.
  }
}
