pub mod external;
pub mod telegram_updates;

use std::{
    cmp::Ordering,
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Arc,
    },
    time::Duration,
};

use crate::external::{
    build_webhook_request_parts, notification_idempotency_key, render_external_notification_text,
    ExternalChannelType, ExternalNotificationSender, ExternalSendOutcome, TelegramChannelConfig,
    WebhookChannelConfig,
};
use chrono::{DateTime, Utc};
use coin_listener_core::{
    models::{
        AddressEvent, NotificationChannel, NotificationOutboxItem, NotificationRule,
        NotifyEventTask, TelegramBindingRequest, TelegramChatBinding,
    },
    AppError, AppResult, NotifyConfig,
};
use coin_listener_storage::{notifications, notifications::ExternalDeliveryStart, repositories};
use sqlx::PgPool;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyChannelDecision {
    InApp,
    External { channel_type: ExternalChannelType },
    Skipped { last_error: &'static str },
}

#[derive(Debug, Clone)]
pub enum ResolvedNotifyChannel {
    Active(NotificationChannel),
    Inactive(uuid::Uuid),
    Missing(uuid::Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyDeliveryPlan {
    pub channel_id: uuid::Uuid,
    pub channel_type: Option<String>,
    pub status: &'static str,
    pub last_error: Option<&'static str>,
    pub create_in_app: bool,
    pub external_channel_type: Option<ExternalChannelType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationOutboxDispatcherConfig {
    pub batch_size: i64,
    pub max_attempts: i32,
    pub stale_lock_seconds: i64,
    pub idle_sleep: Duration,
}

impl NotificationOutboxDispatcherConfig {
    pub fn from_notify_config(config: &NotifyConfig) -> Self {
        Self {
            batch_size: config.outbox_batch_size,
            max_attempts: config.outbox_max_attempts,
            stale_lock_seconds: config.outbox_stale_lock_seconds,
            idle_sleep: Duration::from_millis(config.outbox_idle_sleep_ms),
        }
    }
}

pub const DELIVERY_STATUS_SENT: &str = "sent";
pub const DELIVERY_STATUS_SKIPPED: &str = "skipped";
pub const DELIVERY_STATUS_FAILED: &str = "failed";
pub const DELIVERY_STATUS_PROCESSING: &str = "processing";
pub const NOT_IMPLEMENTED_CHANNEL_ERROR: &str = "channel type not implemented";
pub const UNAVAILABLE_CHANNEL_ERROR: &str = "channel unavailable";
pub const MISSING_CHANNEL_ERROR_PREFIX: &str = "channel missing";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramBindingCandidate {
    pub code: String,
    pub chat: TelegramChatBinding,
}

pub fn notifier_shutdown_requested(shutdown: &AtomicBool) -> bool {
    shutdown.load(AtomicOrdering::Relaxed)
}

pub fn telegram_binding_candidate(
    update: &crate::telegram_updates::TelegramUpdate,
) -> Option<TelegramBindingCandidate> {
    Some(TelegramBindingCandidate {
        code: crate::telegram_updates::extract_binding_code_from_update(update)?,
        chat: crate::telegram_updates::chat_binding_from_update(update)?,
    })
}

pub async fn process_telegram_binding_update(
    pool: &PgPool,
    sender: &external::ExternalNotificationSender,
    telegram_bot_id: uuid::Uuid,
    bot_token: &str,
    update: &crate::telegram_updates::TelegramUpdate,
    now: DateTime<Utc>,
) -> AppResult<Option<TelegramBindingRequest>> {
    let Some(candidate) = telegram_binding_candidate(update) else {
        return Ok(None);
    };

    let Some(binding) = coin_listener_storage::telegram_bindings::bind_pending_request(
        pool,
        telegram_bot_id,
        &candidate.code,
        candidate.chat.clone(),
        now,
    )
    .await?
    else {
        return Ok(None);
    };

    let chat_name = binding
        .chat_title
        .as_deref()
        .or(binding.chat_username.as_deref())
        .or(binding.chat_id.as_deref())
        .unwrap_or("Telegram 会话");
    let outcome = sender
        .send_telegram(
            &external::TelegramChannelConfig {
                telegram_bot_id: Some(telegram_bot_id),
                bot_token_env: None,
                chat_id: binding
                    .chat_id
                    .clone()
                    .unwrap_or_else(|| candidate.chat.chat_id.clone()),
            },
            bot_token,
            &external::telegram_binding_confirmation_text(chat_name),
        )
        .await;

    if !outcome.is_sent() {
        warn!(
            telegram_bot_id = %telegram_bot_id,
            update_id = update.update_id,
            error = ?outcome.metadata().last_error,
            "telegram binding confirmation was not sent"
        );
    }

    Ok(Some(binding))
}

pub fn notification_rule_matches_event(rule: &NotificationRule, event: &AddressEvent) -> bool {
    if rule.tenant_id != event.tenant_id || !rule.enabled {
        return false;
    }
    if rule
        .chain_id
        .is_some_and(|chain_id| chain_id != event.chain_id)
    {
        return false;
    }
    if rule
        .address_id
        .is_some_and(|address_id| address_id != event.address_id)
    {
        return false;
    }
    if rule
        .asset_id
        .is_some_and(|asset_id| asset_id != event.asset_id)
    {
        return false;
    }
    if rule
        .event_type
        .as_ref()
        .is_some_and(|event_type| event_type != &event.event_type)
    {
        return false;
    }
    if rule
        .is_transfer
        .is_some_and(|is_transfer| is_transfer != event.is_transfer)
    {
        return false;
    }
    if rule
        .direction
        .as_ref()
        .is_some_and(|direction| direction != &event.direction)
    {
        return false;
    }

    amount_raw_meets_minimum(event.amount_raw.as_deref(), rule.min_amount_raw.as_deref())
}

pub fn amount_raw_meets_minimum(amount_raw: Option<&str>, min_amount_raw: Option<&str>) -> bool {
    let Some(min_amount_raw) = min_amount_raw else {
        return true;
    };
    let Some(amount_raw) = amount_raw else {
        return false;
    };
    let Some(amount) = normalize_non_negative_integer(amount_raw) else {
        return false;
    };
    let Some(minimum) = normalize_non_negative_integer(min_amount_raw) else {
        return false;
    };

    match amount.len().cmp(&minimum.len()) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => amount >= minimum,
    }
}

pub fn build_in_app_notification_content(event: &AddressEvent) -> (String, String) {
    let title = format!("{} {}", event.event_type, event.direction);
    let amount = event
        .amount_decimal
        .as_deref()
        .or(event.amount_raw.as_deref())
        .unwrap_or("-");
    let tx_hash = event.tx_hash.as_deref().unwrap_or("-");
    let body = format!(
        "address: {}; asset: {}; amount: {}; tx: {}",
        event.address_id, event.asset_id, amount, tx_hash
    );

    (title, body)
}

pub fn notify_channel_decision(channel_type: &str) -> NotifyChannelDecision {
    match channel_type {
        "in_app" => NotifyChannelDecision::InApp,
        "telegram" => NotifyChannelDecision::External {
            channel_type: ExternalChannelType::Telegram,
        },
        "webhook" => NotifyChannelDecision::External {
            channel_type: ExternalChannelType::Webhook,
        },
        _ => NotifyChannelDecision::Skipped {
            last_error: NOT_IMPLEMENTED_CHANNEL_ERROR,
        },
    }
}

pub fn build_delivery_plan(channel: &NotificationChannel) -> NotifyDeliveryPlan {
    match notify_channel_decision(&channel.channel_type) {
        NotifyChannelDecision::InApp => NotifyDeliveryPlan {
            channel_id: channel.id,
            channel_type: Some(channel.channel_type.clone()),
            status: DELIVERY_STATUS_SENT,
            last_error: None,
            create_in_app: true,
            external_channel_type: None,
        },
        NotifyChannelDecision::External { channel_type } => NotifyDeliveryPlan {
            channel_id: channel.id,
            channel_type: Some(channel.channel_type.clone()),
            status: DELIVERY_STATUS_PROCESSING,
            last_error: None,
            create_in_app: false,
            external_channel_type: Some(channel_type),
        },
        NotifyChannelDecision::Skipped { last_error } => NotifyDeliveryPlan {
            channel_id: channel.id,
            channel_type: Some(channel.channel_type.clone()),
            status: DELIVERY_STATUS_SKIPPED,
            last_error: Some(last_error),
            create_in_app: false,
            external_channel_type: None,
        },
    }
}

pub fn build_unavailable_channel_delivery_plan(channel_id: uuid::Uuid) -> NotifyDeliveryPlan {
    NotifyDeliveryPlan {
        channel_id,
        channel_type: None,
        status: DELIVERY_STATUS_SKIPPED,
        last_error: Some(UNAVAILABLE_CHANNEL_ERROR),
        create_in_app: false,
        external_channel_type: None,
    }
}

pub fn missing_channel_error(channel_id: uuid::Uuid) -> String {
    format!("{MISSING_CHANNEL_ERROR_PREFIX}: {channel_id}")
}

pub fn delivery_sent_at(status: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    (status == DELIVERY_STATUS_SENT).then_some(now)
}

pub fn notification_outbox_next_attempt_at(
    now: DateTime<Utc>,
    attempt_count: i32,
) -> DateTime<Utc> {
    let delay_seconds = match attempt_count {
        0 | 1 => 30,
        2 => 60,
        3 => 300,
        4 => 900,
        _ => 3600,
    };
    now + chrono::Duration::seconds(delay_seconds)
}

pub fn notification_outbox_should_fail(attempt_count: i32, max_attempts: i32) -> bool {
    attempt_count >= max_attempts
}

pub fn notification_outbox_task_attempt(attempt_count: i32) -> u16 {
    if attempt_count <= 0 {
        return 0;
    }
    u16::try_from(attempt_count).unwrap_or(u16::MAX)
}

pub fn notify_task_from_outbox_item(item: &NotificationOutboxItem) -> NotifyEventTask {
    NotifyEventTask {
        task_id: item.id,
        event_id: item.event_id,
        tenant_id: item.tenant_id,
        attempt: notification_outbox_task_attempt(item.attempt_count),
        enqueued_at: item.created_at,
    }
}

pub async fn process_notify_task(
    pool: &PgPool,
    task: NotifyEventTask,
    now: DateTime<Utc>,
    sender: &ExternalNotificationSender,
) -> AppResult<usize> {
    let event = notifications::get_address_event(pool, task.event_id, task.tenant_id).await?;
    let rules = notifications::list_enabled_notification_rules(pool, task.tenant_id).await?;
    let mut deliveries = 0usize;

    for rule in rules
        .iter()
        .filter(|rule| notification_rule_matches_event(rule, &event))
    {
        let channels = resolve_rule_channels(pool, task.tenant_id, rule).await?;
        for channel in channels {
            deliveries += delivery_result_count([process_resolved_channel(
                pool, &task, &event, rule, channel, now, sender,
            )
            .await])?;
        }
    }

    Ok(deliveries)
}

pub fn delivery_result_count<I>(results: I) -> AppResult<usize>
where
    I: IntoIterator<Item = AppResult<()>>,
{
    let mut deliveries = 0usize;
    for result in results {
        result?;
        deliveries += 1;
    }
    Ok(deliveries)
}

pub async fn process_notification_outbox_item(
    pool: &PgPool,
    item: NotificationOutboxItem,
    now: DateTime<Utc>,
    sender: &ExternalNotificationSender,
) -> AppResult<usize> {
    process_notify_task(pool, notify_task_from_outbox_item(&item), now, sender).await
}

pub async fn process_notification_outbox_batch(
    pool: &PgPool,
    worker_id: &str,
    config: &NotificationOutboxDispatcherConfig,
    sender: &ExternalNotificationSender,
    now: DateTime<Utc>,
) -> AppResult<usize> {
    let stale_before = now - chrono::Duration::seconds(config.stale_lock_seconds);
    let released = repositories::release_stale_notification_outbox(pool, stale_before, now).await?;
    if released > 0 {
        info!(released, "released stale notification outbox rows");
    }

    let items =
        repositories::claim_due_notification_outbox(pool, now, worker_id, config.batch_size)
            .await?;
    let claimed = items.len();

    for item in items {
        let outbox_id = item.id;
        let event_id = item.event_id;
        let tenant_id = item.tenant_id;
        let attempt_count = item.attempt_count;

        match process_notification_outbox_item(pool, item, now, sender).await {
            Ok(deliveries) => {
                repositories::mark_notification_outbox_delivered(pool, outbox_id, now).await?;
                info!(
                    outbox_id = %outbox_id,
                    event_id = %event_id,
                    tenant_id = %tenant_id,
                    deliveries,
                    "notification outbox item delivered"
                );
            }
            Err(error) => {
                let last_error = error.to_string();
                if notification_outbox_should_fail(attempt_count, config.max_attempts) {
                    repositories::mark_notification_outbox_failed(pool, outbox_id, &last_error)
                        .await?;
                    warn!(
                        outbox_id = %outbox_id,
                        event_id = %event_id,
                        tenant_id = %tenant_id,
                        attempt_count,
                        error = %last_error,
                        "notification outbox item failed permanently"
                    );
                } else {
                    let next_attempt_at = notification_outbox_next_attempt_at(now, attempt_count);
                    repositories::mark_notification_outbox_retryable(
                        pool,
                        outbox_id,
                        next_attempt_at,
                        &last_error,
                    )
                    .await?;
                    warn!(
                        outbox_id = %outbox_id,
                        event_id = %event_id,
                        tenant_id = %tenant_id,
                        attempt_count,
                        next_attempt_at = %next_attempt_at,
                        error = %last_error,
                        "notification outbox item scheduled for retry"
                    );
                }
            }
        }
    }

    Ok(claimed)
}

async fn resolve_rule_channels(
    pool: &PgPool,
    tenant_id: uuid::Uuid,
    rule: &NotificationRule,
) -> AppResult<Vec<ResolvedNotifyChannel>> {
    if rule.channel_ids.is_empty() {
        return Ok(vec![ResolvedNotifyChannel::Active(
            notifications::get_or_create_default_in_app_channel(pool, tenant_id).await?,
        )]);
    }

    let existing_channels =
        notifications::list_channels_by_ids(pool, tenant_id, &rule.channel_ids).await?;
    let active_channels =
        notifications::list_active_channels_by_ids(pool, tenant_id, &rule.channel_ids).await?;

    Ok(resolve_explicit_rule_channels(
        rule.channel_ids.clone(),
        existing_channels,
        active_channels,
    ))
}

pub fn resolve_explicit_rule_channels(
    channel_ids: Vec<uuid::Uuid>,
    existing_channels: Vec<NotificationChannel>,
    active_channels: Vec<NotificationChannel>,
) -> Vec<ResolvedNotifyChannel> {
    let existing_ids = existing_channels
        .into_iter()
        .map(|channel| channel.id)
        .collect::<std::collections::HashSet<_>>();
    let active_by_id: HashMap<uuid::Uuid, NotificationChannel> = active_channels
        .into_iter()
        .map(|channel| (channel.id, channel))
        .collect();

    channel_ids
        .into_iter()
        .map(|channel_id| {
            if let Some(channel) = active_by_id.get(&channel_id) {
                return ResolvedNotifyChannel::Active(channel.clone());
            }
            if existing_ids.contains(&channel_id) {
                ResolvedNotifyChannel::Inactive(channel_id)
            } else {
                ResolvedNotifyChannel::Missing(channel_id)
            }
        })
        .collect()
}

async fn process_resolved_channel(
    pool: &PgPool,
    task: &NotifyEventTask,
    event: &AddressEvent,
    rule: &NotificationRule,
    channel: ResolvedNotifyChannel,
    now: DateTime<Utc>,
    sender: &ExternalNotificationSender,
) -> AppResult<()> {
    match channel {
        ResolvedNotifyChannel::Active(channel) => {
            process_channel_delivery(
                pool,
                sender,
                task,
                event,
                rule,
                build_delivery_plan(&channel),
                now,
            )
            .await
        }
        ResolvedNotifyChannel::Inactive(channel_id) => {
            process_channel_delivery(
                pool,
                sender,
                task,
                event,
                rule,
                build_unavailable_channel_delivery_plan(channel_id),
                now,
            )
            .await
        }
        ResolvedNotifyChannel::Missing(channel_id) => {
            create_skipped_missing_channel_delivery(pool, task, rule, channel_id).await
        }
    }
}

async fn create_skipped_missing_channel_delivery(
    pool: &PgPool,
    task: &NotifyEventTask,
    rule: &NotificationRule,
    channel_id: uuid::Uuid,
) -> AppResult<()> {
    notifications::create_notification_delivery(
        pool,
        task.tenant_id,
        task.event_id,
        Some(rule.id),
        None,
        DELIVERY_STATUS_SKIPPED,
        task.attempt as i32,
        Some(missing_channel_error(channel_id)),
        None,
    )
    .await?;

    Ok(())
}

async fn process_external_channel_delivery(
    pool: &PgPool,
    sender: &ExternalNotificationSender,
    task: &NotifyEventTask,
    event: &AddressEvent,
    rule: &NotificationRule,
    channel_id: uuid::Uuid,
    channel_type: ExternalChannelType,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let idempotency_key =
        notification_idempotency_key(task.tenant_id, task.event_id, rule.id, channel_id);
    let attempt_count = task.attempt as i32;
    let start = notifications::begin_external_notification_delivery(
        pool,
        task.tenant_id,
        task.event_id,
        rule.id,
        channel_id,
        channel_type.as_str(),
        &idempotency_key,
        attempt_count,
    )
    .await?;

    let ExternalDeliveryStart::ReadyToSend { delivery_id } = start else {
        return Ok(());
    };

    let channel = notifications::list_channels_by_ids(pool, task.tenant_id, &[channel_id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::NotFound("external notification channel".to_string()))?;

    let outcome = match channel_type {
        ExternalChannelType::Telegram => {
            let config = match TelegramChannelConfig::parse(&channel.config) {
                Ok(config) => config,
                Err(error) => {
                    notifications::mark_external_notification_delivery_failed(
                        pool,
                        task.tenant_id,
                        delivery_id,
                        attempt_count,
                        &error.message,
                        None,
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };
            let bot_token = if let Some(bot_id) = config.telegram_bot_id {
                match notifications::get_telegram_bot_secret(pool, task.tenant_id, bot_id).await {
                    Ok(bot) if bot.status == "active" => bot.bot_token,
                    Ok(_) => {
                        notifications::mark_external_notification_delivery_failed(
                            pool,
                            task.tenant_id,
                            delivery_id,
                            attempt_count,
                            "telegram bot is inactive",
                            None,
                            None,
                        )
                        .await?;
                        return Ok(());
                    }
                    Err(error) => {
                        let message = error.to_string();
                        notifications::mark_external_notification_delivery_failed(
                            pool,
                            task.tenant_id,
                            delivery_id,
                            attempt_count,
                            &message,
                            None,
                            None,
                        )
                        .await?;
                        return Ok(());
                    }
                }
            } else {
                match config
                    .bot_token_env
                    .as_deref()
                    .and_then(|name| std::env::var(name).ok())
                {
                    Some(token) => token,
                    None => {
                        notifications::mark_external_notification_delivery_failed(
                            pool,
                            task.tenant_id,
                            delivery_id,
                            attempt_count,
                            "telegram token env is not set",
                            None,
                            None,
                        )
                        .await?;
                        return Ok(());
                    }
                }
            };
            sender
                .send_telegram(
                    &config,
                    &bot_token,
                    &render_external_notification_text(event),
                )
                .await
        }
        ExternalChannelType::Webhook => {
            let config = match WebhookChannelConfig::parse(&channel.config) {
                Ok(config) => config,
                Err(error) => {
                    notifications::mark_external_notification_delivery_failed(
                        pool,
                        task.tenant_id,
                        delivery_id,
                        attempt_count,
                        &error.message,
                        None,
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };
            let secret = match load_optional_env_secret(config.secret_env.as_deref()) {
                Ok(secret) => secret,
                Err(message) => {
                    notifications::mark_external_notification_delivery_failed(
                        pool,
                        task.tenant_id,
                        delivery_id,
                        attempt_count,
                        &message,
                        None,
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };
            let parts = match build_webhook_request_parts(
                &config,
                event,
                &idempotency_key,
                secret.as_deref(),
            ) {
                Ok(parts) => parts,
                Err(error) => {
                    notifications::mark_external_notification_delivery_failed(
                        pool,
                        task.tenant_id,
                        delivery_id,
                        attempt_count,
                        &error.message,
                        None,
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };
            sender.send_webhook(parts).await
        }
    };

    let metadata = outcome.metadata();
    match &outcome {
        ExternalSendOutcome::Sent(_) => {
            notifications::mark_external_notification_delivery_sent(
                pool,
                task.tenant_id,
                delivery_id,
                attempt_count,
                now,
                metadata.provider_message_id.as_deref(),
                metadata.provider_status_code,
                metadata.provider_response.as_deref(),
            )
            .await?;
        }
        ExternalSendOutcome::PermanentFailure(_) | ExternalSendOutcome::TransientFailure(_) => {
            notifications::mark_external_notification_delivery_failed(
                pool,
                task.tenant_id,
                delivery_id,
                attempt_count,
                metadata
                    .last_error
                    .as_deref()
                    .unwrap_or("external notification failed"),
                metadata.provider_status_code,
                metadata.provider_response.as_deref(),
            )
            .await?;
        }
    }

    external_send_outcome_result(&outcome)
}

fn load_optional_env_secret(secret_env: Option<&str>) -> Result<Option<String>, String> {
    match secret_env {
        Some(secret_env) => std::env::var(secret_env)
            .map(Some)
            .map_err(|_| format!("webhook secret env {secret_env} is not set")),
        None => Ok(None),
    }
}

async fn process_channel_delivery(
    pool: &PgPool,
    sender: &ExternalNotificationSender,
    task: &NotifyEventTask,
    event: &AddressEvent,
    rule: &NotificationRule,
    plan: NotifyDeliveryPlan,
    now: DateTime<Utc>,
) -> AppResult<()> {
    if plan.create_in_app {
        let (title, body) = build_in_app_notification_content(event);
        sent_in_app_delivery_result(
            notifications::create_sent_in_app_delivery(
                pool,
                task.tenant_id,
                task.event_id,
                Some(rule.id),
                Some(plan.channel_id),
                task.attempt as i32,
                now,
                title,
                body,
            )
            .await,
        )?;
        return Ok(());
    }

    if let Some(external_channel_type) = plan.external_channel_type {
        return process_external_channel_delivery(
            pool,
            sender,
            task,
            event,
            rule,
            plan.channel_id,
            external_channel_type,
            now,
        )
        .await;
    }

    notifications::create_notification_delivery(
        pool,
        task.tenant_id,
        task.event_id,
        Some(rule.id),
        Some(plan.channel_id),
        plan.status,
        task.attempt as i32,
        plan.last_error.map(str::to_string),
        delivery_sent_at(plan.status, now),
    )
    .await?;

    Ok(())
}

pub fn external_send_outcome_result(outcome: &ExternalSendOutcome) -> AppResult<()> {
    match outcome {
        ExternalSendOutcome::Sent(_) | ExternalSendOutcome::PermanentFailure(_) => Ok(()),
        ExternalSendOutcome::TransientFailure(metadata) => Err(AppError::ExternalNotification(
            metadata
                .last_error
                .clone()
                .unwrap_or_else(|| "external notification transient failure".to_string()),
        )),
    }
}

pub fn sent_in_app_delivery_result<T>(result: AppResult<T>) -> AppResult<()> {
    result.map(|_| ())
}

pub fn notifier_batch_claimed_count(result: AppResult<usize>) -> usize {
    match result {
        Ok(claimed) => claimed,
        Err(error) => {
            warn!(error = %error, "notification outbox batch failed");
            0
        }
    }
}

pub async fn run_notifier(
    pool: PgPool,
    config: NotificationOutboxDispatcherConfig,
    sender: ExternalNotificationSender,
    shutdown: Arc<AtomicBool>,
) -> AppResult<()> {
    let worker_id = format!("notifier-{}", uuid::Uuid::new_v4());

    while !notifier_shutdown_requested(&shutdown) {
        let claimed = notifier_batch_claimed_count(
            process_notification_outbox_batch(&pool, &worker_id, &config, &sender, Utc::now())
                .await,
        );
        if claimed == 0 {
            tokio::time::sleep(config.idle_sleep).await;
        }
    }

    Ok(())
}

fn normalize_non_negative_integer(value: &str) -> Option<&str> {
    if value.is_empty() || !value.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let normalized = value.trim_start_matches('0');
    if normalized.is_empty() {
        Some("0")
    } else {
        Some(normalized)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use chrono::{TimeZone, Utc};
    use coin_listener_core::{
        models::{AddressEvent, NotificationChannel, NotificationOutboxItem, NotificationRule},
        AppError, NotifyConfig,
    };
    use serde_json::json;

    use crate::external::{ExternalChannelType, ExternalSendMetadata, ExternalSendOutcome};
    use crate::{
        amount_raw_meets_minimum, build_delivery_plan, build_in_app_notification_content,
        build_unavailable_channel_delivery_plan, delivery_result_count, delivery_sent_at,
        external_send_outcome_result, missing_channel_error, notification_outbox_next_attempt_at,
        notification_outbox_should_fail, notification_outbox_task_attempt,
        notification_rule_matches_event, notifier_batch_claimed_count, notifier_shutdown_requested,
        notify_channel_decision, notify_task_from_outbox_item, resolve_explicit_rule_channels,
        sent_in_app_delivery_result, NotificationOutboxDispatcherConfig, NotifyChannelDecision,
        ResolvedNotifyChannel, DELIVERY_STATUS_PROCESSING, DELIVERY_STATUS_SENT,
        DELIVERY_STATUS_SKIPPED, MISSING_CHANNEL_ERROR_PREFIX, UNAVAILABLE_CHANNEL_ERROR,
    };

    fn uuid(value: u128) -> sqlx::types::Uuid {
        sqlx::types::Uuid::from_u128(value)
    }

    fn event() -> AddressEvent {
        AddressEvent {
            id: uuid(1),
            tenant_id: uuid(2),
            chain_id: uuid(3),
            address_id: uuid(4),
            asset_id: uuid(5),
            event_type: "transfer".to_string(),
            direction: "in".to_string(),
            is_transfer: true,
            tx_hash: Some("0xabc".to_string()),
            log_index: Some(0),
            block_number: Some(100),
            block_hash: None,
            confirmations: 12,
            from_address: Some("0xfrom".to_string()),
            to_address: Some("0xto".to_string()),
            amount_raw: Some("1000".to_string()),
            amount_decimal: Some("0.000000000000001".to_string()),
            balance_before_raw: None,
            balance_after_raw: None,
            balance_delta_raw: None,
            metadata: Default::default(),
            detected_at: Utc.with_ymd_and_hms(2026, 5, 17, 17, 0, 0).unwrap(),
            created_at: Utc.with_ymd_and_hms(2026, 5, 17, 17, 0, 1).unwrap(),
        }
    }

    fn outbox_item(attempt_count: i32) -> NotificationOutboxItem {
        NotificationOutboxItem {
            id: uuid(90),
            tenant_id: uuid(2),
            event_id: uuid(1),
            status: "processing".to_string(),
            attempt_count,
            next_attempt_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap(),
            locked_at: Some(Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 1).unwrap()),
            locked_by: Some("notifier-test".to_string()),
            last_error: None,
            delivered_at: None,
            created_at: Utc.with_ymd_and_hms(2026, 5, 18, 11, 59, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 1).unwrap(),
        }
    }

    fn channel(channel_type: &str) -> NotificationChannel {
        channel_with_id(uuid(20), channel_type)
    }

    fn channel_with_id(id: sqlx::types::Uuid, channel_type: &str) -> NotificationChannel {
        NotificationChannel {
            id,
            tenant_id: uuid(2),
            channel_type: channel_type.to_string(),
            name: format!("{channel_type} channel"),
            config: Default::default(),
            status: "active".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 5, 17, 16, 30, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 5, 17, 16, 30, 0).unwrap(),
        }
    }

    fn rule() -> NotificationRule {
        NotificationRule {
            id: uuid(10),
            tenant_id: uuid(2),
            name: "Inbound transfers".to_string(),
            chain_id: Some(uuid(3)),
            address_id: Some(uuid(4)),
            asset_id: Some(uuid(5)),
            event_type: Some("transfer".to_string()),
            is_transfer: Some(true),
            min_amount_raw: Some("1000".to_string()),
            direction: Some("in".to_string()),
            channel_ids: vec![],
            enabled: true,
            created_at: Utc.with_ymd_and_hms(2026, 5, 17, 16, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 5, 17, 16, 0, 0).unwrap(),
        }
    }

    #[test]
    fn binding_processor_ignores_update_without_code() {
        let update = crate::telegram_updates::TelegramUpdate {
            update_id: 200,
            message: Some(crate::telegram_updates::TelegramMessage {
                text: Some("hello bot".to_string()),
                chat: crate::telegram_updates::TelegramChat {
                    id: json!(12345),
                    chat_type: "private".to_string(),
                    title: None,
                    username: Some("alice".to_string()),
                    first_name: Some("Alice".to_string()),
                    last_name: None,
                },
            }),
            channel_post: None,
        };

        assert_eq!(crate::telegram_binding_candidate(&update), None);
    }

    #[test]
    fn binding_processor_extracts_code_and_chat_candidate() {
        let update = crate::telegram_updates::TelegramUpdate {
            update_id: 201,
            message: Some(crate::telegram_updates::TelegramMessage {
                text: Some("please bind CL-7K2P9Q".to_string()),
                chat: crate::telegram_updates::TelegramChat {
                    id: json!(-1001234567890_i64),
                    chat_type: "supergroup".to_string(),
                    title: Some("Ops Alerts".to_string()),
                    username: Some("ops_alerts".to_string()),
                    first_name: None,
                    last_name: None,
                },
            }),
            channel_post: None,
        };

        let candidate = crate::telegram_binding_candidate(&update).expect("binding candidate");

        assert_eq!(candidate.code, "CL-7K2P9Q");
        assert_eq!(candidate.chat.chat_id, "-1001234567890");
        assert_eq!(candidate.chat.chat_type, "supergroup");
        assert_eq!(candidate.chat.chat_title.as_deref(), Some("Ops Alerts"));
        assert_eq!(candidate.chat.chat_username.as_deref(), Some("ops_alerts"));
    }

    #[test]
    fn rule_matches_when_all_filters_match() {
        assert!(notification_rule_matches_event(&rule(), &event()));
    }

    #[test]
    fn notification_outbox_backoff_is_deterministic() {
        let now = Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap();

        assert_eq!(
            notification_outbox_next_attempt_at(now, 1),
            now + chrono::Duration::seconds(30)
        );
        assert_eq!(
            notification_outbox_next_attempt_at(now, 2),
            now + chrono::Duration::seconds(60)
        );
        assert_eq!(
            notification_outbox_next_attempt_at(now, 3),
            now + chrono::Duration::seconds(300)
        );
        assert_eq!(
            notification_outbox_next_attempt_at(now, 4),
            now + chrono::Duration::seconds(900)
        );
        assert_eq!(
            notification_outbox_next_attempt_at(now, 5),
            now + chrono::Duration::seconds(3600)
        );
    }

    #[test]
    fn notification_outbox_retry_policy_fails_at_max_attempts() {
        assert!(!notification_outbox_should_fail(9, 10));
        assert!(notification_outbox_should_fail(10, 10));
        assert!(notification_outbox_should_fail(11, 10));
    }

    #[test]
    fn dispatcher_config_is_loaded_from_notify_config() {
        let notify = NotifyConfig {
            queue_key: "notify:event:queue".to_string(),
            outbox_batch_size: 25,
            outbox_max_attempts: 7,
            outbox_stale_lock_seconds: 120,
            outbox_idle_sleep_ms: 250,
            telegram_webhook_secret: None,
        };

        let config = NotificationOutboxDispatcherConfig::from_notify_config(&notify);

        assert_eq!(config.batch_size, 25);
        assert_eq!(config.max_attempts, 7);
        assert_eq!(config.stale_lock_seconds, 120);
        assert_eq!(config.idle_sleep, std::time::Duration::from_millis(250));
    }

    #[test]
    fn notifier_loop_uses_outbox_repository_instead_of_redis_dequeue() {
        let source = include_str!("lib.rs");
        let forbidden = ["notify_queue", ".dequeue"].join("");

        assert!(source.contains("claim_due_notification_outbox"));
        assert!(source.contains("release_stale_notification_outbox"));
        assert!(source.contains("mark_notification_outbox_delivered"));
        assert!(source.contains("mark_notification_outbox_retryable"));
        assert!(source.contains("mark_notification_outbox_failed"));
        assert!(!source.contains(&forbidden));
    }

    #[test]
    fn notification_outbox_task_attempt_clamps_to_notify_task_type() {
        assert_eq!(notification_outbox_task_attempt(1), 1);
        assert_eq!(notification_outbox_task_attempt(i32::MAX), u16::MAX);
        assert_eq!(notification_outbox_task_attempt(-1), 0);
    }

    #[test]
    fn outbox_item_converts_to_legacy_notify_task_for_processing() {
        let item = outbox_item(3);
        let task = notify_task_from_outbox_item(&item);

        assert_eq!(task.task_id, item.id);
        assert_eq!(task.event_id, item.event_id);
        assert_eq!(task.tenant_id, item.tenant_id);
        assert_eq!(task.attempt, 3);
        assert_eq!(task.enqueued_at, item.created_at);
    }

    #[test]
    fn delivery_result_count_propagates_processing_errors() {
        let results = [
            Ok(()),
            Err(AppError::Database("delivery write failed".to_string())),
        ];

        let error = delivery_result_count(results).expect_err("delivery failure should propagate");

        assert!(error.to_string().contains("delivery write failed"));
    }

    #[test]
    fn delivery_result_count_counts_successful_business_skips() {
        let results = [Ok(()), Ok(())];

        assert_eq!(delivery_result_count(results).unwrap(), 2);
    }

    #[test]
    fn sent_in_app_delivery_result_propagates_atomic_write_error() {
        let error = AppError::NotFound("in-app notification target".to_string());

        let result = sent_in_app_delivery_result(Err::<(), _>(error));

        assert!(matches!(
            result,
            Err(AppError::NotFound(entity)) if entity == "in-app notification target"
        ));
    }

    #[test]
    fn rule_does_not_match_when_amount_is_below_minimum() {
        let mut event = event();
        event.amount_raw = Some("999".to_string());

        assert!(!notification_rule_matches_event(&rule(), &event));
    }

    #[test]
    fn amount_comparison_handles_large_integer_strings() {
        assert!(amount_raw_meets_minimum(
            Some("100000000000000000000"),
            Some("99999999999999999999")
        ));
        assert!(amount_raw_meets_minimum(Some("00100"), Some("100")));
        assert!(!amount_raw_meets_minimum(Some("99"), Some("100")));
        assert!(!amount_raw_meets_minimum(None, Some("1")));
        assert!(!amount_raw_meets_minimum(Some("abc"), Some("1")));
    }

    #[test]
    fn in_app_content_uses_stable_event_fields() {
        let (title, body) = build_in_app_notification_content(&event());

        assert_eq!(title, "transfer in");
        assert!(body.contains("address: 00000000-0000-0000-0000-000000000004"));
        assert!(body.contains("asset: 00000000-0000-0000-0000-000000000005"));
        assert!(body.contains("amount: 0.000000000000001"));
        assert!(body.contains("tx: 0xabc"));
    }

    #[test]
    fn notifier_treats_telegram_and_webhook_as_sendable_channels() {
        assert_eq!(
            notify_channel_decision("telegram"),
            NotifyChannelDecision::External {
                channel_type: ExternalChannelType::Telegram
            }
        );
        assert_eq!(
            notify_channel_decision("webhook"),
            NotifyChannelDecision::External {
                channel_type: ExternalChannelType::Webhook
            }
        );
    }

    #[test]
    fn unsupported_channel_is_skipped() {
        assert_eq!(
            notify_channel_decision("email"),
            NotifyChannelDecision::Skipped {
                last_error: "channel type not implemented"
            }
        );
    }

    #[test]
    fn in_app_channel_delivery_plan_creates_sent_notification() {
        let plan = build_delivery_plan(&channel("in_app"));

        assert_eq!(plan.status, DELIVERY_STATUS_SENT);
        assert_eq!(plan.last_error, None);
        assert!(plan.create_in_app);
    }

    #[test]
    fn external_channel_delivery_plan_records_sendable_channel_type() {
        let telegram = build_delivery_plan(&channel("telegram"));
        let webhook = build_delivery_plan(&channel("webhook"));

        assert_eq!(telegram.status, DELIVERY_STATUS_PROCESSING);
        assert_eq!(
            telegram.external_channel_type,
            Some(ExternalChannelType::Telegram)
        );
        assert!(!telegram.create_in_app);
        assert_eq!(webhook.status, DELIVERY_STATUS_PROCESSING);
        assert_eq!(
            webhook.external_channel_type,
            Some(ExternalChannelType::Webhook)
        );
        assert!(!webhook.create_in_app);
    }

    #[test]
    fn unsupported_channel_delivery_plan_is_skipped_without_in_app() {
        let plan = build_delivery_plan(&channel("email"));

        assert_eq!(plan.status, DELIVERY_STATUS_SKIPPED);
        assert_eq!(plan.last_error, Some("channel type not implemented"));
        assert!(!plan.create_in_app);
        assert_eq!(plan.external_channel_type, None);
    }

    #[test]
    fn transient_external_send_error_keeps_outbox_retryable() {
        let outcome = ExternalSendOutcome::TransientFailure(ExternalSendMetadata {
            last_error: Some("webhook returned retryable status 429".to_string()),
            provider_message_id: None,
            provider_status_code: Some(429),
            provider_response: Some("rate limited".to_string()),
        });

        let result = external_send_outcome_result(&outcome);

        assert!(matches!(
            result,
            Err(AppError::ExternalNotification(message)) if message.contains("retryable status 429")
        ));
    }

    #[test]
    fn permanent_external_send_error_does_not_trigger_outbox_retry() {
        let outcome = ExternalSendOutcome::PermanentFailure(ExternalSendMetadata {
            last_error: Some("webhook returned permanent status 401".to_string()),
            provider_message_id: None,
            provider_status_code: Some(401),
            provider_response: Some("unauthorized".to_string()),
        });

        assert!(external_send_outcome_result(&outcome).is_ok());
    }

    #[test]
    fn sent_at_is_only_set_for_sent_delivery() {
        let now = Utc.with_ymd_and_hms(2026, 5, 17, 18, 30, 0).unwrap();

        assert_eq!(delivery_sent_at(DELIVERY_STATUS_SENT, now), Some(now));
        assert_eq!(delivery_sent_at(DELIVERY_STATUS_SKIPPED, now), None);
    }

    #[test]
    fn explicit_channel_resolution_preserves_inactive_and_missing_audit_entries() {
        let inactive = channel_with_id(uuid(20), "in_app");
        let active = channel_with_id(uuid(21), "in_app");
        let resolved = resolve_explicit_rule_channels(
            vec![uuid(20), uuid(21), uuid(22)],
            vec![inactive.clone(), active.clone()],
            vec![active.clone()],
        );

        assert_eq!(resolved.len(), 3);
        assert!(matches!(resolved[0], ResolvedNotifyChannel::Inactive(id) if id == uuid(20)));
        match &resolved[1] {
            ResolvedNotifyChannel::Active(channel) => assert_eq!(channel.id, active.id),
            other => panic!("expected active channel, got {other:?}"),
        }
        assert!(matches!(resolved[2], ResolvedNotifyChannel::Missing(id) if id == uuid(22)));
    }

    #[test]
    fn missing_channel_error_includes_channel_id_for_audit() {
        let error = missing_channel_error(uuid(23));

        assert!(error.starts_with(MISSING_CHANNEL_ERROR_PREFIX));
        assert!(error.contains("00000000-0000-0000-0000-000000000017"));
    }

    #[test]
    fn unavailable_channel_delivery_plan_is_skipped_with_audit_error() {
        let plan = build_unavailable_channel_delivery_plan(uuid(22));

        assert_eq!(plan.channel_id, uuid(22));
        assert_eq!(plan.status, DELIVERY_STATUS_SKIPPED);
        assert_eq!(plan.last_error, Some(UNAVAILABLE_CHANNEL_ERROR));
        assert!(!plan.create_in_app);
    }

    #[test]
    fn set_shutdown_flag_stops_notifier_before_next_dequeue() {
        let shutdown = AtomicBool::new(true);

        assert!(notifier_shutdown_requested(&shutdown));
    }

    #[test]
    fn notifier_batch_claimed_count_keeps_loop_alive_after_error() {
        let error = AppError::Database("claim failed".to_string());

        assert_eq!(notifier_batch_claimed_count(Err(error)), 0);
    }
}
