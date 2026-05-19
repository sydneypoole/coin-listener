use axum::extract::ws::{Message, WebSocket};
use chrono::{DateTime, Utc};
use coin_listener_core::{models::InAppNotification, AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::{broadcast, RwLock},
    time,
};
use uuid::Uuid;

pub const REALTIME_NOTIFICATION_CHANNEL: &str = "coin_listener_in_app_notifications";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum RealtimeServerMessage {
    #[serde(rename = "in_app_notification.created")]
    InAppNotificationCreated(InAppNotification),
    #[serde(rename = "ping")]
    Ping { sent_at: DateTime<Utc> },
}

impl RealtimeServerMessage {
    pub fn message_type(&self) -> &'static str {
        match self {
            Self::InAppNotificationCreated(_) => "in_app_notification.created",
            Self::Ping { .. } => "ping",
        }
    }

    pub fn tenant_id(&self) -> Uuid {
        match self {
            Self::InAppNotificationCreated(notification) => notification.tenant_id,
            Self::Ping { .. } => Uuid::nil(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RealtimeHub {
    inner: Arc<RwLock<HashMap<Uuid, broadcast::Sender<RealtimeServerMessage>>>>,
    buffer_size: usize,
}

impl RealtimeHub {
    pub fn new(buffer_size: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            buffer_size,
        }
    }

    pub async fn subscribe_async(
        &self,
        tenant_id: Uuid,
    ) -> broadcast::Receiver<RealtimeServerMessage> {
        let mut channels = self.inner.write().await;
        let sender = channels.entry(tenant_id).or_insert_with(|| {
            let (sender, _) = broadcast::channel(self.buffer_size);
            sender
        });

        sender.subscribe()
    }

    pub fn subscribe(&self, tenant_id: Uuid) -> broadcast::Receiver<RealtimeServerMessage> {
        let mut channels = self.inner.blocking_write();
        let sender = channels.entry(tenant_id).or_insert_with(|| {
            let (sender, _) = broadcast::channel(self.buffer_size);
            sender
        });

        sender.subscribe()
    }

    pub fn broadcast(&self, message: RealtimeServerMessage) -> usize {
        let channels = self.inner.blocking_read();
        channels
            .get(&message.tenant_id())
            .map(|sender| sender.send(message).unwrap_or(0))
            .unwrap_or(0)
    }

    pub async fn broadcast_async(&self, message: RealtimeServerMessage) -> usize {
        let channels = self.inner.read().await;
        channels
            .get(&message.tenant_id())
            .map(|sender| sender.send(message).unwrap_or(0))
            .unwrap_or(0)
    }
}

pub fn notification_message(notification: InAppNotification) -> RealtimeServerMessage {
    RealtimeServerMessage::InAppNotificationCreated(notification)
}

pub fn realtime_token_from_query(query: &str) -> AppResult<&str> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
        .filter(|token| !token.is_empty())
        .ok_or(AppError::Unauthorized)
}

pub async fn websocket_connection(mut socket: WebSocket, hub: RealtimeHub, tenant_id: Uuid) {
    let mut receiver = hub.subscribe_async(tenant_id).await;
    let mut ping_interval = time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            message = receiver.recv() => {
                let Ok(message) = message else {
                    break;
                };
                if send_json(&mut socket, &message).await.is_err() {
                    break;
                }
            }
            _ = ping_interval.tick() => {
                let message = RealtimeServerMessage::Ping { sent_at: Utc::now() };
                if send_json(&mut socket, &message).await.is_err() {
                    break;
                }
            }
            client_message = socket.recv() => {
                match client_message {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

async fn send_json(socket: &mut WebSocket, message: &RealtimeServerMessage) -> AppResult<()> {
    let text =
        serde_json::to_string(message).map_err(|error| AppError::Validation(error.to_string()))?;
    socket
        .send(Message::Text(text))
        .await
        .map_err(|error| AppError::Validation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        notification_message, realtime_token_from_query, RealtimeHub, RealtimeServerMessage,
    };
    use chrono::Utc;
    use coin_listener_core::models::InAppNotification;
    use uuid::Uuid;

    fn notification_for_tenant(tenant_id: Uuid) -> InAppNotification {
        InAppNotification {
            id: Uuid::from_u128(10),
            tenant_id,
            event_id: Uuid::from_u128(11),
            delivery_id: Some(Uuid::from_u128(12)),
            title: "transfer in".to_string(),
            body: "address: watched; asset: ETH; amount: 1; tx: 0x1".to_string(),
            read_at: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn realtime_token_query_extracts_non_empty_token() {
        assert_eq!(
            realtime_token_from_query("token=abc.def.ghi").expect("token present"),
            "abc.def.ghi"
        );
        assert!(realtime_token_from_query("token=").is_err());
        assert!(realtime_token_from_query("other=value").is_err());
    }

    #[test]
    fn notification_message_uses_stable_type() {
        let notification = notification_for_tenant(Uuid::from_u128(1));
        let message = notification_message(notification.clone());

        assert!(matches!(
            message,
            RealtimeServerMessage::InAppNotificationCreated(_)
        ));
        assert_eq!(message.message_type(), "in_app_notification.created");
        assert_eq!(message.tenant_id(), notification.tenant_id);
    }

    #[test]
    fn realtime_hub_broadcasts_only_to_matching_tenant() {
        let hub = RealtimeHub::new(16);
        let tenant_a = Uuid::from_u128(1);
        let tenant_b = Uuid::from_u128(2);
        let mut tenant_a_rx = hub.subscribe(tenant_a);
        let mut tenant_b_rx = hub.subscribe(tenant_b);
        let message = notification_message(notification_for_tenant(tenant_a));

        assert_eq!(hub.broadcast(message), 1);
        assert!(tenant_a_rx.try_recv().is_ok());
        assert!(tenant_b_rx.try_recv().is_err());
    }
}
