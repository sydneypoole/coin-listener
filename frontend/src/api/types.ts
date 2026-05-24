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
  asset_ids: string[];
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

export type EvmTransactionRescanRequest = {
  chain_id: string;
  tx_hash: string;
};

export type EvmTransactionRescanTransferSummary = {
  asset_id: string;
  symbol: string;
  token_contract: string;
  from_address: string;
  to_address: string;
  amount_raw: string;
  amount_decimal: string;
  log_index: number;
};

export type EvmTransactionRescanSummary = {
  chain_id: string;
  tx_hash: string;
  tx_from: string;
  tx_to?: string | null;
  native_value_raw: string;
  block_number: number;
  token_transfer_count: number;
  inserted_event_count: number;
  skipped_event_count: number;
};

export type EvmTransactionRescanResponse = {
  summary: EvmTransactionRescanSummary;
  token_transfers: EvmTransactionRescanTransferSummary[];
  events: AddressEvent[];
};

export type CreateProviderRequest = Omit<Provider, 'id'>;

export type ProviderTestResponse = {
  ok: boolean;
  message: string;
  latest_block?: number | null;
  chain_type: string;
  provider_type: string;
};

export type CreateWatchedAddressRequest = Omit<WatchedAddress, 'id' | 'tenant_id'> & {
  tenant_id?: string;
};

export type CustodyAccountSource = 'pool' | 'user' | string;
export type CustodyAccountStatus = 'available' | 'assigned' | 'disabled' | string;
export type CustodyAssignmentStatus = 'active' | 'released' | 'cancelled' | string;
export type CustodyApplicantType = 'api' | 'internal' | string;

export type CustodyAccountChainConfigRequest = {
  chain_id: string;
  asset_ids: string[];
};

export type CustodyAccountChainConfig = {
  id: string;
  chain_id: string;
  chain_name: string;
  asset_ids: string[];
  asset_symbols: string[];
};

export type CustodyAssignmentWatchedAddress = {
  chain_id: string;
  chain_name: string;
  watched_address_id: string;
  asset_ids: string[];
};

export type CustodyAccount = {
  id: string;
  tenant_id: string;
  chain_id: string;
  chain_name: string;
  address: string;
  label?: string | null;
  source: CustodyAccountSource;
  status: CustodyAccountStatus;
  watched_address_id?: string | null;
  current_assignment_id?: string | null;
  current_business_ref?: string | null;
  chain_configs: CustodyAccountChainConfig[];
  created_at: string;
  updated_at: string;
};

export type CustodyAccountAssignment = {
  id: string;
  tenant_id: string;
  custody_account_id: string;
  chain_id: string;
  chain_name: string;
  address: string;
  applicant_type: CustodyApplicantType;
  business_ref: string;
  purpose?: string | null;
  status: CustodyAssignmentStatus;
  watched_address_id?: string | null;
  assigned_at: string;
  released_at?: string | null;
  created_at: string;
  updated_at: string;
};

export type CreateCustodyAccountRequest = {
  chain_id: string;
  address: string;
  label?: string | null;
  source: CustodyAccountSource;
  status?: CustodyAccountStatus;
  chain_configs: CustodyAccountChainConfigRequest[];
};

export type AssignCustodyAccountRequest = {
  chain_id?: string | null;
  source: CustodyAccountSource;
  address?: string | null;
  applicant_type: CustodyApplicantType;
  business_ref: string;
  purpose?: string | null;
  chain_configs?: CustodyAccountChainConfigRequest[] | null;
};

export type CustodyAccountQuery = {
  chain_id?: string;
  source?: CustodyAccountSource;
  status?: CustodyAccountStatus;
};

export type CustodyAssignmentQuery = {
  chain_id?: string;
  status?: CustodyAssignmentStatus;
  business_ref?: string;
};

export type AssignCustodyAccountResponse = {
  account: CustodyAccount;
  assignment: CustodyAccountAssignment;
  watched_addresses: CustodyAssignmentWatchedAddress[];
};

export type TelegramBot = {
  id: string;
  tenant_id: string;
  name: string;
  token_preview: string;
  proxy_source: string;
  proxy_url_preview?: string | null;
  status: string;
  verification_status: string;
  last_verified_at?: string | null;
  last_error?: string | null;
  created_at: string;
  updated_at: string;
};

export type TelegramSettings = {
  tenant_id: string;
  proxy_url_preview?: string | null;
  has_proxy: boolean;
  created_at?: string | null;
  updated_at?: string | null;
};

export type UpdateTelegramSettingsRequest = {
  proxy_url?: string | null;
};

export type CreateTelegramBotRequest = {
  name: string;
  bot_token: string;
  status?: string;
  proxy_url?: string | null;
};

export type UpdateTelegramBotRequest = {
  name: string;
  bot_token?: string | null;
  status: string;
  proxy_url?: string | null;
};

export type TelegramBindingRequest = {
  id: string;
  tenant_id: string;
  telegram_bot_id: string;
  status: 'pending' | 'bound' | 'expired' | 'cancelled' | string;
  bind_token: string;
  short_code: string;
  deep_link_url?: string | null;
  chat_id?: string | null;
  chat_type?: string | null;
  chat_title?: string | null;
  chat_username?: string | null;
  confirmation_error?: string | null;
  expires_at: string;
  bound_at?: string | null;
  created_at: string;
  updated_at: string;
};

export type CreateTelegramBindingRequest = {
  telegram_bot_id: string;
};

export type VerificationResponse = {
  ok: boolean;
  message: string;
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

export type UpdateNotificationChannelRequest = {
  channel_type: string;
  name: string;
  config?: Record<string, unknown>;
  status: string;
};

export type NotificationChannelTestResponse = {
  ok: boolean;
  message: string;
};

export type WatchedAddressImportChainConfig = {
  chain_id: string;
  asset_ids: string[];
};

export type WatchedAddressImportDefaults = {
  chain_id: string;
  asset_ids: string[];
  chain_configs: WatchedAddressImportChainConfig[];
  priority: string;
  scan_interval_seconds: number;
  transfer_filter_enabled: boolean;
  balance_change_filter_enabled: boolean;
  status: string;
};

export type WatchedAddressImportRowRequest = {
  row_number: number;
  raw_text: string;
  address: string;
  label?: string | null;
  priority?: string | null;
  scan_interval_seconds?: number | null;
  transfer_filter_enabled?: boolean | null;
  balance_change_filter_enabled?: boolean | null;
  status?: string | null;
};

export type CreateWatchedAddressImportRequest = {
  defaults: WatchedAddressImportDefaults;
  rows: WatchedAddressImportRowRequest[];
};

export type WatchedAddressImportTask = {
  id: string;
  tenant_id: string;
  status: string;
  chain_id: string;
  asset_ids: string[];
  chain_configs: WatchedAddressImportChainConfig[];
  priority: string;
  scan_interval_seconds: number;
  transfer_filter_enabled: boolean;
  balance_change_filter_enabled: boolean;
  address_status: string;
  total_rows: number;
  processed_rows: number;
  success_rows: number;
  failed_rows: number;
  locked_at?: string | null;
  locked_by?: string | null;
  started_at?: string | null;
  completed_at?: string | null;
  last_error?: string | null;
  created_at: string;
  updated_at: string;
};

export type WatchedAddressImportErrorRow = {
  row_number: number;
  address: string;
  raw_text: string;
  chain_id: string;
  chain_name?: string | null;
  error_code?: string | null;
  error_message?: string | null;
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

export type RealtimeNotificationCreatedMessage = {
  type: 'in_app_notification.created';
  payload: InAppNotification;
};

export type RealtimePingMessage = {
  type: 'ping';
  payload: { sent_at: string };
};

export type RealtimeServerMessage = RealtimeNotificationCreatedMessage | RealtimePingMessage;

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

export type NotificationOutboxItem = {
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
};

export type NotificationOutboxListItem = NotificationOutboxItem & {
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
  outbox: NotificationOutboxItem;
};

export type ScanRunStatus = 'running' | 'success' | 'failed' | 'locked' | 'unsupported' | string;

export type ScanAddressTask = {
  task_id: string;
  address_id: string;
  tenant_id: string;
  chain_id: string;
  attempt: number;
  enqueued_at: string;
};

export type ScanRunListItem = {
  id: string;
  tenant_id: string;
  task_id: string;
  address_id: string;
  chain_id: string;
  chain_name: string;
  address: string;
  address_label?: string | null;
  chain_type: string;
  status: ScanRunStatus;
  event_count: number;
  started_at: string;
  finished_at?: string | null;
  duration_ms?: number | null;
  error_message?: string | null;
};

export type ScanRunDetail = ScanRunListItem & {
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
};

export type ScanRunQuery = {
  chain_id?: string;
  address_id?: string;
  status?: ScanRunStatus;
  started_after?: string;
  started_before?: string;
  limit?: number;
  offset?: number;
};

export type ScanRunListResponse = {
  items: ScanRunListItem[];
  limit: number;
  offset: number;
};

export type RetryScanRunResponse = {
  task: ScanAddressTask;
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
  last_success_at?: string | null;
  last_failed_at?: string | null;
  last_24h_success: number;
  last_24h_failed: number;
  recent_runs: ScanRunListItem[];
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

export type ProviderHealthStatus = {
  consecutive_failures: number;
  last_success_at?: string | null;
  last_failure_at?: string | null;
  disabled_until?: string | null;
  last_error?: string | null;
  is_circuit_open: boolean;
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
  health: ProviderHealthStatus;
};

export type ProviderStatus = {
  active: number;
  inactive: number;
  by_chain: ProviderChainStatus[];
  items: ProviderStatusItem[];
};

export type ServiceHeartbeatStatusItem = {
  service_name: string;
  instance_id: string;
  status: string;
  started_at: string;
  last_seen_at: string;
  stale_after_seconds: number;
  is_stale: boolean;
  metadata: Record<string, unknown>;
};

export type ServiceHealthStatus = {
  online: number;
  stale: number;
  items: ServiceHeartbeatStatusItem[];
};

export type SystemStatus = {
  generated_at: string;
  queues: QueueStatus;
  scans: ScanStatus;
  events: EventStatus;
  notifications: NotificationStatus;
  providers: ProviderStatus;
  services: ServiceHealthStatus;
};
