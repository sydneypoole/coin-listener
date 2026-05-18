export type UserSummary = {
  id: string;
  email: string;
  display_name: string;
};

export type Tenant = {
  id: string;
  name: string;
  status: string;
};

export type LoginResponse = {
  token: string;
  user: UserSummary;
  tenant: Tenant;
};

export type Chain = {
  id: string;
  key: string;
  name: string;
  chain_type: string;
  native_asset_symbol: string;
  status: string;
  default_confirmations: number;
};

export type Asset = {
  id: string;
  chain_id: string;
  asset_type: string;
  symbol: string;
  name: string;
  contract_address?: string | null;
  decimals: number;
  is_builtin: boolean;
  status: string;
};

export type Provider = {
  id: string;
  chain_id: string;
  provider_type: string;
  name: string;
  base_url: string;
  api_key_ref?: string | null;
  priority: number;
  qps_limit: number;
  timeout_ms: number;
  status: string;
};

export type WatchedAddress = {
  id: string;
  tenant_id: string;
  chain_id: string;
  address: string;
  label?: string | null;
  priority: string;
  scan_interval_seconds: number;
  transfer_filter_enabled: boolean;
  balance_change_filter_enabled: boolean;
  status: string;
};

export type AddressEvent = {
  id: string;
  tenant_id: string;
  chain_id: string;
  address_id: string;
  asset_id: string;
  event_type: string;
  direction: string;
  is_transfer: boolean;
  tx_hash?: string | null;
  log_index?: number | null;
  block_number?: number | null;
  block_hash?: string | null;
  confirmations: number;
  from_address?: string | null;
  to_address?: string | null;
  amount_raw?: string | null;
  amount_decimal?: string | null;
  balance_before_raw?: string | null;
  balance_after_raw?: string | null;
  balance_delta_raw?: string | null;
  metadata: Record<string, unknown>;
  detected_at: string;
  created_at: string;
};

export type EventQuery = {
  chain_id?: string;
  address_id?: string;
  asset_id?: string;
  event_type?: string;
  direction?: string;
  is_transfer?: boolean;
};

export type CreateProviderRequest = Omit<Provider, 'id'>;
export type CreateWatchedAddressRequest = Omit<WatchedAddress, 'id' | 'tenant_id'> & {
  tenant_id?: string;
};

export type NotificationChannel = {
  id: string;
  tenant_id: string;
  channel_type: string;
  name: string;
  config: Record<string, unknown>;
  status: string;
  created_at: string;
  updated_at: string;
};

export type CreateNotificationChannelRequest = {
  channel_type: string;
  name: string;
  config?: Record<string, unknown>;
  status?: string;
};

export type NotificationRule = {
  id: string;
  tenant_id: string;
  name: string;
  chain_id?: string | null;
  address_id?: string | null;
  asset_id?: string | null;
  event_type?: string | null;
  is_transfer?: boolean | null;
  min_amount_raw?: string | null;
  direction?: string | null;
  channel_ids: string[];
  enabled: boolean;
  created_at: string;
  updated_at: string;
};

export type CreateNotificationRuleRequest = {
  name: string;
  chain_id?: string | null;
  address_id?: string | null;
  asset_id?: string | null;
  event_type?: string | null;
  is_transfer?: boolean | null;
  min_amount_raw?: string | null;
  direction?: string | null;
  channel_ids?: string[];
  enabled?: boolean;
};

export type InAppNotification = {
  id: string;
  tenant_id: string;
  event_id: string;
  delivery_id?: string | null;
  title: string;
  body: string;
  read_at?: string | null;
  created_at: string;
};

export type InAppNotificationQuery = {
  unread_only?: boolean;
};

export type OutboxStatusCounts = {
  pending: number;
  retryable: number;
  processing: number;
  failed: number;
  stale_processing: number;
  next_due_at?: string | null;
};

export type NotificationOutboxQuery = {
  status?: string;
  event_id?: string;
  limit?: number;
  offset?: number;
};

export type NotificationOutboxListItem = {
  id: string;
  tenant_id: string;
  event_id: string;
  status: string;
  attempt_count: number;
  next_attempt_at: string;
  locked_at?: string | null;
  locked_by?: string | null;
  last_error?: string | null;
  delivered_at?: string | null;
  created_at: string;
  updated_at: string;
  event_type?: string | null;
  direction?: string | null;
  tx_hash?: string | null;
  delivery_total: number;
  delivery_sent: number;
  delivery_failed: number;
  delivery_skipped: number;
  is_stale_processing: boolean;
};

export type NotificationDeliveryQuery = {
  event_id?: string;
  status?: string;
  channel_type?: string;
  rule_id?: string;
  channel_id?: string;
  limit?: number;
  offset?: number;
};

export type NotificationDeliveryListItem = {
  id: string;
  tenant_id: string;
  event_id: string;
  rule_id?: string | null;
  channel_id?: string | null;
  channel_type?: string | null;
  status: string;
  attempt_count: number;
  last_error?: string | null;
  sent_at?: string | null;
  created_at: string;
  idempotency_key?: string | null;
  provider_message_id?: string | null;
  provider_status_code?: number | null;
  provider_response?: string | null;
};

export type NotificationOutboxListResponse = {
  items: NotificationOutboxListItem[];
  limit: number;
  offset: number;
};

export type NotificationOutboxDetail = {
  outbox: NotificationOutboxListItem;
  event: AddressEvent;
  deliveries: NotificationDeliveryListItem[];
};

export type NotificationDeliveryListResponse = {
  items: NotificationDeliveryListItem[];
  limit: number;
  offset: number;
};

export type RetryNotificationOutboxResponse = {
  outbox: NotificationOutboxListItem;
};

export type QueueStatus = {
  scan_queue_key: string;
  scan_queue_depth?: number | null;
  notify_queue_key: string;
  notify_queue_depth?: number | null;
  queue_errors: string[];
};

export type ScanStatus = {
  active_addresses: number;
  due_addresses: number;
  overdue_addresses: number;
  last_scanned_at?: string | null;
};

export type EventStatus = {
  last_24h_total: number;
  last_24h_transfers: number;
  last_24h_non_transfers: number;
};

export type NotificationStatus = {
  last_24h_sent: number;
  last_24h_skipped: number;
  last_24h_failed: number;
  unread_in_app: number;
  outbox: OutboxStatusCounts;
};

export type ProviderChainStatus = {
  chain_id: string;
  chain_name: string;
  active: number;
  inactive: number;
};

export type ProviderStatusItem = {
  id: string;
  chain_id: string;
  chain_name: string;
  provider_type: string;
  name: string;
  base_url: string;
  priority: number;
  qps_limit: number;
  timeout_ms: number;
  status: string;
};

export type ProviderStatus = {
  active: number;
  inactive: number;
  by_chain: ProviderChainStatus[];
  items: ProviderStatusItem[];
};

export type SystemStatus = {
  generated_at: string;
  queues: QueueStatus;
  scans: ScanStatus;
  events: EventStatus;
  notifications: NotificationStatus;
  providers: ProviderStatus;
};
