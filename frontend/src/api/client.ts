import type {
  AddressEvent,
  Asset,
  Chain,
  CreateNotificationChannelRequest,
  CreateNotificationRuleRequest,
  CreateProviderRequest,
  CreateWatchedAddressRequest,
  EventQuery,
  InAppNotification,
  InAppNotificationQuery,
  LoginResponse,
  NotificationChannel,
  NotificationDeliveryListResponse,
  NotificationDeliveryQuery,
  NotificationOutboxDetail,
  NotificationOutboxListResponse,
  NotificationOutboxQuery,
  NotificationRule,
  Provider,
  RetryNotificationOutboxResponse,
  SystemStatus,
  WatchedAddress,
} from './types';

const apiBaseUrl = import.meta.env.VITE_API_BASE_URL ?? '';

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const response = await fetch(`${apiBaseUrl}${path}`, {
    headers: {
      'Content-Type': 'application/json',
      ...options.headers,
    },
    ...options,
  });

  if (!response.ok) {
    const body = await response.json().catch(() => ({ error: response.statusText }));
    throw new Error(body.error ?? response.statusText);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return response.json();
}

function buildQuery(filters: object): string {
  const params = new URLSearchParams();
  Object.entries(filters).forEach(([key, value]) => {
    if (value !== undefined && value !== null && value !== '') {
      params.set(key, String(value));
    }
  });
  const query = params.toString();
  return query ? `?${query}` : '';
}

export function login(email: string, password: string): Promise<LoginResponse> {
  return request<LoginResponse>('/api/auth/login', {
    method: 'POST',
    body: JSON.stringify({ email, password }),
  });
}

export function listChains(): Promise<Chain[]> {
  return request<Chain[]>('/api/chains');
}

export function listAssets(): Promise<Asset[]> {
  return request<Asset[]>('/api/assets');
}

export function listProviders(): Promise<Provider[]> {
  return request<Provider[]>('/api/providers');
}

export function createProvider(payload: CreateProviderRequest): Promise<Provider> {
  return request<Provider>('/api/providers', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function listWatchedAddresses(): Promise<WatchedAddress[]> {
  return request<WatchedAddress[]>('/api/addresses');
}

export function createWatchedAddress(payload: CreateWatchedAddressRequest): Promise<WatchedAddress> {
  return request<WatchedAddress>('/api/addresses', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function updateWatchedAddress(id: string, payload: CreateWatchedAddressRequest): Promise<WatchedAddress> {
  return request<WatchedAddress>(`/api/addresses/${id}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteWatchedAddress(id: string): Promise<void> {
  return request<void>(`/api/addresses/${id}`, {
    method: 'DELETE',
  });
}

export function listEvents(filters: EventQuery = {}): Promise<AddressEvent[]> {
  return request<AddressEvent[]>(`/api/events${buildQuery(filters)}`);
}

export function scanAddress(id: string): Promise<AddressEvent> {
  return request<AddressEvent>(`/api/dev/scan-address/${id}`, {
    method: 'POST',
  });
}

export function listNotificationChannels(): Promise<NotificationChannel[]> {
  return request<NotificationChannel[]>('/api/notification-channels');
}

export function createNotificationChannel(payload: CreateNotificationChannelRequest): Promise<NotificationChannel> {
  return request<NotificationChannel>('/api/notification-channels', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function listNotificationRules(): Promise<NotificationRule[]> {
  return request<NotificationRule[]>('/api/notification-rules');
}

export function createNotificationRule(payload: CreateNotificationRuleRequest): Promise<NotificationRule> {
  return request<NotificationRule>('/api/notification-rules', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function updateNotificationRule(id: string, payload: CreateNotificationRuleRequest): Promise<NotificationRule> {
  return request<NotificationRule>(`/api/notification-rules/${id}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteNotificationRule(id: string): Promise<void> {
  return request<void>(`/api/notification-rules/${id}`, {
    method: 'DELETE',
  });
}

export function listInAppNotifications(filters: InAppNotificationQuery = {}): Promise<InAppNotification[]> {
  return request<InAppNotification[]>(`/api/in-app-notifications${buildQuery(filters)}`);
}

export function markInAppNotificationRead(id: string): Promise<InAppNotification> {
  return request<InAppNotification>(`/api/in-app-notifications/${id}/read`, {
    method: 'POST',
  });
}

export function listNotificationOutbox(filters: NotificationOutboxQuery = {}): Promise<NotificationOutboxListResponse> {
  return request<NotificationOutboxListResponse>(`/api/notification-outbox${buildQuery(filters)}`);
}

export function getNotificationOutbox(id: string): Promise<NotificationOutboxDetail> {
  return request<NotificationOutboxDetail>(`/api/notification-outbox/${id}`);
}

export function retryNotificationOutbox(id: string): Promise<RetryNotificationOutboxResponse> {
  return request<RetryNotificationOutboxResponse>(`/api/notification-outbox/${id}/retry`, {
    method: 'POST',
  });
}

export function listNotificationDeliveries(filters: NotificationDeliveryQuery = {}): Promise<NotificationDeliveryListResponse> {
  return request<NotificationDeliveryListResponse>(`/api/notification-deliveries${buildQuery(filters)}`);
}

export function getSystemStatus(): Promise<SystemStatus> {
  return request<SystemStatus>('/api/system/status');
}
