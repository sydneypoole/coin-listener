use coin_listener_core::models::TelegramChatBinding;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub channel_post: Option<TelegramMessage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramMessage {
    pub text: Option<String>,
    pub chat: TelegramChat,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramChat {
    pub id: serde_json::Value,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

pub fn extract_binding_code(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|part| {
        let token = part.trim_matches(|ch: char| ch == ',' || ch == '.' || ch == ':' || ch == ';');
        if token.starts_with("bind_") || token.starts_with("CL-") {
            return Some(token.to_string());
        }
        None
    })
}

pub fn extract_binding_code_from_update(update: &TelegramUpdate) -> Option<String> {
    extract_binding_code(update_message(update)?.text.as_deref()?)
}

pub fn chat_binding_from_update(update: &TelegramUpdate) -> Option<TelegramChatBinding> {
    let chat = &update_message(update)?.chat;
    let chat_id = match &chat.id {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Number(value) => value.to_string(),
        _ => return None,
    };
    let chat_title = chat.title.clone().or_else(|| {
        let name = [chat.first_name.as_deref(), chat.last_name.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        (!name.is_empty()).then_some(name)
    });
    Some(TelegramChatBinding {
        chat_id,
        chat_type: chat.chat_type.clone(),
        chat_title,
        chat_username: chat.username.clone(),
    })
}

fn update_message(update: &TelegramUpdate) -> Option<&TelegramMessage> {
    update.message.as_ref().or(update.channel_post.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_private_start_bind_token() {
        assert_eq!(
            extract_binding_code("/start bind_abc123"),
            Some("bind_abc123".to_string())
        );
    }

    #[test]
    fn extracts_group_short_code_with_or_without_bot_mention() {
        assert_eq!(
            extract_binding_code("@coin_listener_bot CL-7K2P9Q"),
            Some("CL-7K2P9Q".to_string())
        );
        assert_eq!(
            extract_binding_code("please bind CL-7K2P9Q now"),
            Some("CL-7K2P9Q".to_string())
        );
    }

    #[test]
    fn ignores_messages_without_binding_code() {
        assert_eq!(extract_binding_code("hello bot"), None);
    }

    #[test]
    fn extracts_channel_post_binding_code() {
        let update: TelegramUpdate = serde_json::from_value(json!({
            "update_id": 100,
            "channel_post": {
                "text": "CL-CH99",
                "chat": { "id": "-1009876543210", "type": "channel" }
            }
        }))
        .expect("telegram channel post update");

        assert_eq!(
            extract_binding_code_from_update(&update),
            Some("CL-CH99".to_string())
        );
    }

    #[test]
    fn maps_group_chat_to_binding_metadata() {
        let update = TelegramUpdate {
            update_id: 99,
            message: Some(TelegramMessage {
                text: Some("CL-7K2P9Q".to_string()),
                chat: TelegramChat {
                    id: json!(-1001234567890_i64),
                    chat_type: "supergroup".to_string(),
                    title: Some("Ops Alerts".to_string()),
                    username: None,
                    first_name: None,
                    last_name: None,
                },
            }),
            channel_post: None,
        };

        let binding = chat_binding_from_update(&update).expect("chat binding");

        assert_eq!(binding.chat_id, "-1001234567890");
        assert_eq!(binding.chat_type, "supergroup");
        assert_eq!(binding.chat_title.as_deref(), Some("Ops Alerts"));
    }

    #[test]
    fn maps_channel_post_chat_to_binding_metadata() {
        let update: TelegramUpdate = serde_json::from_value(json!({
            "update_id": 100,
            "channel_post": {
                "text": "CL-CH99",
                "chat": {
                    "id": "-1009876543210",
                    "type": "channel",
                    "title": "Channel Alerts",
                    "username": "channel_alerts"
                }
            }
        }))
        .expect("telegram channel post update");

        let binding = chat_binding_from_update(&update).expect("channel post binding");

        assert_eq!(binding.chat_id, "-1009876543210");
        assert_eq!(binding.chat_type, "channel");
        assert_eq!(binding.chat_title.as_deref(), Some("Channel Alerts"));
        assert_eq!(binding.chat_username.as_deref(), Some("channel_alerts"));
    }
}
