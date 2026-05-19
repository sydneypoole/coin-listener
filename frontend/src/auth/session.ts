import type { LoginResponse } from '../api/types';

const SESSION_STORAGE_KEY = 'coin-listener.session.v1';
let currentSession: LoginResponse | null = null;
let unauthorizedHandler: (() => void) | null = null;

export function loadStoredSession(): LoginResponse | null {
  if (typeof window === 'undefined') return currentSession;

  const raw = window.localStorage.getItem(SESSION_STORAGE_KEY);
  if (!raw) {
    currentSession = null;
    return null;
  }

  try {
    currentSession = JSON.parse(raw) as LoginResponse;
    return currentSession;
  } catch {
    window.localStorage.removeItem(SESSION_STORAGE_KEY);
    currentSession = null;
    return null;
  }
}

export function saveSession(session: LoginResponse): void {
  currentSession = session;
  if (typeof window !== 'undefined') {
    window.localStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(session));
  }
}

export function clearSession(): void {
  currentSession = null;
  if (typeof window !== 'undefined') {
    window.localStorage.removeItem(SESSION_STORAGE_KEY);
  }
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
  unauthorizedHandler?.();
}
