use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, stream::SplitSink};
use uuid::Uuid;

use crate::{
    models::chat::ChatServerMessage,
    state::{ChatConnectionInfo, ChatConnectionInfoMap, RedisClient},
    ws::handlers::utils::queue_message_for_player,
};
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn store_chat_connection_and_send_queued_messages(
    player_id: Uuid,
    sender: SplitSink<WebSocket, Message>,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let sender = Arc::new(Mutex::new(sender.into()));
    let connection_info = Arc::new(ChatConnectionInfo {
        sender: sender.clone(),
    });

    {
        let mut conn_map = chat_connections.lock().await;
        conn_map.insert(player_id, connection_info);
    }

    // Send any queued messages for this player
    if let Ok(queued_messages) = get_queued_messages_for_player(player_id, redis).await {
        let mut sender_guard = sender.lock().await;
        for message in queued_messages {
            if let Err(e) = sender_guard.send(Message::Text(message.into())).await {
                tracing::error!(
                    "Failed to send queued message to player {}: {}",
                    player_id,
                    e
                );
                break;
            }
        }
    }
}

pub async fn remove_chat_connection(player_id: Uuid, chat_connections: &ChatConnectionInfoMap) {
    let mut conn_map = chat_connections.lock().await;
    if conn_map.remove(&player_id).is_some() {
        tracing::info!("Removed chat connection for player {}", player_id);
    }
}

pub async fn send_chat_message_to_player(
    player_id: Uuid,
    msg: &ChatServerMessage,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let serialized = match serde_json::to_string(msg) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize chat message: {}", e);
            return;
        }
    };

    let conns = chat_connections.lock().await;
    if let Some(conn_info) = conns.get(&player_id) {
        let mut sender = conn_info.sender.lock().await;
        if let Err(e) = sender.send(Message::Text(serialized.clone().into())).await {
            tracing::error!("Failed to send chat message to player {}: {}", player_id, e);

            if msg.should_queue() {
                drop(sender);
                drop(conns);

                if let Err(queue_err) = queue_message_for_player(player_id, serialized, redis).await
                {
                    tracing::error!(
                        "Failed to queue chat message for player {}: {}",
                        player_id,
                        queue_err
                    );
                }
            }
        }
    } else if msg.should_queue() {
        if let Err(e) = queue_message_for_player(player_id, serialized, redis).await {
            tracing::error!(
                "Failed to queue chat message for offline player {}: {}",
                player_id,
                e
            );
        }
    }
}

async fn get_queued_messages_for_player(
    player_id: Uuid,
    redis: &RedisClient,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    // This function should be implemented to retrieve queued messages from Redis
    // For now, returning empty vector as placeholder
    Ok(vec![])
}
