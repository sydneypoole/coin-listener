import { getAuthRequestContext, handleUnauthorized } from '../auth/session';
import type {
  AddressEvent,
  Asset,
  Chain,
  CreateNotificationChannelRequest,
  CreateNotificationRuleRequest,
  CreateProviderRequest,
  CreateTelegramBindingRequest,
  CreateTelegramBotRequest,
  CreateWatchedAddressImportRequest,
  CreateWatchedAddressRequest,
  EventQuery,
  EvmTransactionRescanRequest,
  EvmTransactionRescanResponse,
  InAppNotification,
  InAppNotificationQuery,
  LoginResponse,
  NotificationChannel,
  NotificationChannelTestResponse,
  NotificationDeliveryListResponse,
  NotificationDeliveryQuery,
  NotificationOutboxDetail,
  NotificationOutboxListResponse,
  NotificationOutboxQuery,
  NotificationRule,
  Provider,
  ProviderTestResponse,
  RetryNotificationOutboxResponse,
  SystemStatus,
  TelegramBindingRequest,
  TelegramBot,
  TelegramSettings,
  UpdateNotificationChannelRequest,
  UpdateTelegramBotRequest,
  UpdateTelegramSettingsRequest,
  VerificationResponse,
  WatchedAddress,
  WatchedAddressImportErrorRow,
  WatchedAddressImportTask,
} from './types';

const apiBaseUrl = import.meta.env.VITE_API_BASE_URL ?? '';

export class ApiRequestError extends Error {
  constructor(message: string, readonly status: number) {
    super(message);
    this.name = 'ApiRequestError';
  }
}

export async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const isLoginRequest = path === '/api/auth/login';
  const headers = new Headers(options.headers);
  headers.set('Content-Type', 'application/json');

  const authContext = isLoginRequest ? null : getAuthRequestContext();
  if (authContext?.token) {
    headers.set('Authorization', `Bearer ${authContext.token}`);
  }

  const response = await fetch(`${apiBaseUrl}${path}`, {
    ...options,
    headers,
  });

  if (!response.ok) {
    if (authContext && response.status === 401) {
      handleUnauthorized(authContext);
    }
    const body = await response.json().catch(() => ({ error: response.statusText }));
    throw new ApiRequestError(body.error ?? response.statusText, response.status);
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

export function updateProvider(id: string, payload: CreateProviderRequest): Promise<Provider> {
  return request<Provider>(`/api/providers/${id}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function testProvider(id: string): Promise<ProviderTestResponse> {
  return request<ProviderTestResponse>(`/api/providers/${id}/test`, {
    method: 'POST',
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

export function rescanEvmTransaction(payload: EvmTransactionRescanRequest): Promise<EvmTransactionRescanResponse> {
  return request<EvmTransactionRescanResponse>('/api/evm/transactions/rescan', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function scanAddress(id: string): Promise<AddressEvent> {
  return request<AddressEvent>(`/api/dev/scan-address/${id}`, {
    method: 'POST',
  });
}

export function createWatchedAddressImport(payload: CreateWatchedAddressImportRequest): Promise<WatchedAddressImportTask> {
  return request<WatchedAddressImportTask>('/api/addresses/imports', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function getWatchedAddressImport(id: string): Promise<WatchedAddressImportTask> {
  return request<WatchedAddressImportTask>(`/api/addresses/imports/${id}`);
}

export function listWatchedAddressImportErrors(id: string): Promise<WatchedAddressImportErrorRow[]> {
  return request<WatchedAddressImportErrorRow[]>(`/api/addresses/imports/${id}/errors`);
}

export function cancelWatchedAddressImport(id: string): Promise<WatchedAddressImportTask> {
  return request<WatchedAddressImportTask>(`/api/addresses/imports/${id}/cancel`, {
    method: 'POST',
  });
}

export function getTelegramSettings(): Promise<TelegramSettings> {
  return request<TelegramSettings>('/api/telegram-settings');
}

export function updateTelegramSettings(payload: UpdateTelegramSettingsRequest): Promise<TelegramSettings> {
  return request<TelegramSettings>('/api/telegram-settings', {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function listTelegramBots(): Promise<TelegramBot[]> {
  return request<TelegramBot[]>('/api/telegram-bots');
}

export function createTelegramBot(payload: CreateTelegramBotRequest): Promise<TelegramBot> {
  return request<TelegramBot>('/api/telegram-bots', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function updateTelegramBot(id: string, payload: UpdateTelegramBotRequest): Promise<TelegramBot> {
  return request<TelegramBot>(`/api/telegram-bots/${id}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteTelegramBot(id: string): Promise<void> {
  return request<void>(`/api/telegram-bots/${id}`, {
    method: 'DELETE',
  });
}

export function verifyTelegramBot(id: string): Promise<VerificationResponse> {
  return request<VerificationResponse>(`/api/telegram-bots/${id}/verify`, {
    method: 'POST',
  });
}

export function createTelegramBinding(payload: CreateTelegramBindingRequest): Promise<TelegramBindingRequest> {
  return request<TelegramBindingRequest>('/api/telegram-bindings', {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

export function getTelegramBinding(id: string): Promise<TelegramBindingRequest> {
  return request<TelegramBindingRequest>(`/api/telegram-bindings/${id}`);
}

export function cancelTelegramBinding(id: string): Promise<TelegramBindingRequest> {
  return request<TelegramBindingRequest>(`/api/telegram-bindings/${id}/cancel`, {
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

export function updateNotificationChannel(
  id: string,
  payload: UpdateNotificationChannelRequest,
): Promise<NotificationChannel> {
  return request<NotificationChannel>(`/api/notification-channels/${id}`, {
    method: 'PUT',
    body: JSON.stringify(payload),
  });
}

export function deleteNotificationChannel(id: string): Promise<void> {
  return request<void>(`/api/notification-channels/${id}`, {
    method: 'DELETE',
  });
}

export function verifyNotificationChannel(id: string): Promise<VerificationResponse> {
  return request<VerificationResponse>(`/api/notification-channels/${id}/verify`, {
    method: 'POST',
  });
}

export function testNotificationChannel(id: string): Promise<NotificationChannelTestResponse> {
  return request<NotificationChannelTestResponse>(`/api/notification-channels/${id}/test`, {
    method: 'POST',
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
