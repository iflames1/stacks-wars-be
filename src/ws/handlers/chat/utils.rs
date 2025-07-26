use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, stream::SplitSink};
use uuid::Uuid;

use crate::{
    errors::AppError,
    models::chat::ChatServerMessage,
    state::{ChatConnectionInfo, ChatConnectionInfoMap, RedisClient},
};
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn queue_chat_message_for_player(
    player_id: Uuid,
    message: String,
    redis: &RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("chat_queue:{}", player_id);

    // Use Redis list to store messages with 5-minute TTL
    let _: () = redis::cmd("LPUSH")
        .arg(&key)
        .arg(&message)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    // Set TTL to 5 minutes (300 seconds)
    let _: () = redis::cmd("EXPIRE")
        .arg(&key)
        .arg(300)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    tracing::debug!("Queued chat message for player {} in Redis", player_id);
    Ok(())
}

pub async fn get_queued_chat_messages_for_player(
    player_id: Uuid,
    redis: &RedisClient,
) -> Result<Vec<String>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("chat_queue:{}", player_id);

    let messages: Vec<String> = redis::cmd("LRANGE")
        .arg(&key)
        .arg(0)
        .arg(-1)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    if !messages.is_empty() {
        let _: () = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;

        tracing::info!(
            "Retrieved {} queued chat messages for player {}",
            messages.len(),
            player_id
        );
    }

    Ok(messages.into_iter().rev().collect())
}

async fn store_chat_connection(
    player_id: Uuid,
    sender: SplitSink<WebSocket, Message>,
    chat_connections: &ChatConnectionInfoMap,
) {
    let mut conns = chat_connections.lock().await;
    let conn_info = ChatConnectionInfo {
        sender: Arc::new(Mutex::new(sender)),
    };
    conns.insert(player_id, Arc::new(conn_info));
    tracing::debug!("Stored chat connection for player {}", player_id);
}

pub async fn store_chat_connection_and_send_queued_messages(
    player_id: Uuid,
    sender: SplitSink<WebSocket, Message>,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    // Store the chat connection first
    store_chat_connection(player_id, sender, chat_connections).await;

    // Check for queued chat messages and send them
    match get_queued_chat_messages_for_player(player_id, redis).await {
        Ok(messages) => {
            if !messages.is_empty() {
                tracing::info!(
                    "Sending {} queued chat messages to player {}",
                    messages.len(),
                    player_id
                );

                let conns = chat_connections.lock().await;
                if let Some(conn_info) = conns.get(&player_id) {
                    let mut sender_guard = conn_info.sender.lock().await;

                    let mut sent_count = 0;
                    for message in messages {
                        if let Err(e) = sender_guard.send(Message::Text(message.into())).await {
                            tracing::error!(
                                "Failed to send queued chat message to player {}: {}",
                                player_id,
                                e
                            );
                            break;
                        }
                        sent_count += 1;

                        // Small delay to avoid overwhelming the client
                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                    }

                    tracing::info!(
                        "Successfully sent {} queued chat messages to player {}",
                        sent_count,
                        player_id
                    );
                }
            }
        }
        Err(e) => {
            tracing::error!(
                "Failed to retrieve queued chat messages for player {}: {}",
                player_id,
                e
            );
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

                if let Err(queue_err) =
                    queue_chat_message_for_player(player_id, serialized, redis).await
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
        if let Err(e) = queue_chat_message_for_player(player_id, serialized, redis).await {
            tracing::error!(
                "Failed to queue chat message for offline player {}: {}",
                player_id,
                e
            );
        }
    }
}
