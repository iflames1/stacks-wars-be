use axum::extract::ws::Message;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use uuid::Uuid;

use crate::{
    db,
    models::{
        chat::{ChatClientMessage, ChatMessage, ChatServerMessage},
        game::Player,
    },
    state::{ChatConnectionInfoMap, ChatHistories, RedisClient},
    ws::handlers::chat::utils::{queue_chat_message_for_player, send_chat_message_to_player},
};

pub async fn handle_incoming_chat_messages(
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    room_id: Uuid,
    player: &Player,
    chat_connections: &ChatConnectionInfoMap,
    redis: RedisClient,
    chat_histories: ChatHistories,
) {
    while let Some(msg_result) = receiver.next().await {
        match msg_result {
            Ok(msg) => match msg {
                Message::Text(text) => {
                    if let Ok(parsed) = serde_json::from_str::<ChatClientMessage>(&text) {
                        match parsed {
                            ChatClientMessage::Ping { ts } => {
                                let now = Utc::now().timestamp_millis() as u64;
                                let pong_time = now.saturating_sub(ts);

                                let pong_msg = ChatServerMessage::Pong {
                                    ts,
                                    pong: pong_time,
                                };
                                send_chat_message_to_player(
                                    player.id,
                                    &pong_msg,
                                    chat_connections,
                                    &redis,
                                )
                                .await;
                            }
                            ChatClientMessage::Chat { text } => {
                                let room_players = match db::room::get_room_players(
                                    room_id,
                                    redis.clone(),
                                )
                                .await
                                {
                                    Ok(players) => players,
                                    Err(e) => {
                                        tracing::error!("Failed to get room players: {}", e);
                                        // Send error to user and continue
                                        let error_msg = ChatServerMessage::Error {
                                            message: "Failed to verify room membership".to_string(),
                                        };
                                        send_chat_message_to_player(
                                            player.id,
                                            &error_msg,
                                            chat_connections,
                                            &redis,
                                        )
                                        .await;
                                        continue;
                                    }
                                };

                                let is_room_member = room_players.iter().any(|p| p.id == player.id);

                                if !is_room_member {
                                    let error_msg = ChatServerMessage::Error {
                                        message: "You are not a member of this room".to_string(),
                                    };
                                    send_chat_message_to_player(
                                        player.id,
                                        &error_msg,
                                        chat_connections,
                                        &redis,
                                    )
                                    .await;
                                    continue;
                                }

                                if text.trim().is_empty() {
                                    let error_msg = ChatServerMessage::Error {
                                        message: "Message cannot be empty".to_string(),
                                    };
                                    send_chat_message_to_player(
                                        player.id,
                                        &error_msg,
                                        chat_connections,
                                        &redis,
                                    )
                                    .await;
                                    continue;
                                }

                                // Create chat message
                                let chat_message = ChatMessage {
                                    id: Uuid::new_v4(),
                                    text: text.trim().to_string(),
                                    sender: player.clone(),
                                    timestamp: Utc::now(),
                                };

                                // Store in chat history
                                {
                                    let mut histories = chat_histories.lock().await;
                                    let history = histories
                                        .entry(room_id)
                                        .or_insert_with(|| crate::state::ChatHistory::new());
                                    history.add_message(chat_message.clone());
                                }

                                broadcast_chat_to_room(
                                    &chat_message,
                                    &room_players,
                                    chat_connections,
                                    &redis,
                                )
                                .await;
                            }
                        }
                    }
                }
                Message::Ping(data) => {
                    // Handle WebSocket ping with custom timestamp logic
                    if data.len() == 8 {
                        let mut ts_bytes = [0u8; 8];
                        if data.len() >= 8 {
                            ts_bytes.copy_from_slice(&data[..8]);
                        }
                        let client_ts = u64::from_le_bytes(ts_bytes);

                        let now = Utc::now().timestamp_millis() as u64;
                        let pong_time = now.saturating_sub(client_ts);

                        let pong_msg = ChatServerMessage::Pong {
                            ts: client_ts,
                            pong: pong_time,
                        };
                        send_chat_message_to_player(player.id, &pong_msg, chat_connections, &redis)
                            .await;
                    } else {
                        let now = Utc::now().timestamp_millis() as u64;
                        let pong_msg = ChatServerMessage::Pong { ts: now, pong: 0 };
                        send_chat_message_to_player(player.id, &pong_msg, chat_connections, &redis)
                            .await;
                    }
                }
                Message::Pong(_) => {
                    tracing::debug!("Received pong from player {}", player.id);
                }
                Message::Close(_) => {
                    tracing::info!("Player {} closed chat connection", player.wallet_address);
                    break;
                }
                _ => {}
            },
            Err(e) => {
                tracing::error!(
                    "Chat WebSocket error for player {}: {}",
                    player.wallet_address,
                    e
                );
                break;
            }
        }
    }
}

async fn broadcast_chat_to_room(
    chat_message: &ChatMessage,
    room_players: &[Player],
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let chat_msg = ChatServerMessage::Chat {
        message: chat_message.clone(),
    };
    let serialized = match serde_json::to_string(&chat_msg) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize chat message: {}", e);
            return;
        }
    };

    let connection_guard = chat_connections.lock().await;

    for player in room_players {
        if let Some(conn_info) = connection_guard.get(&player.id) {
            let mut sender = conn_info.sender.lock().await;
            if let Err(e) = sender.send(Message::Text(serialized.clone().into())).await {
                tracing::warn!("Failed to send chat message to player {}: {}", player.id, e);

                if chat_msg.should_queue() {
                    drop(sender);
                    drop(connection_guard);

                    if let Err(queue_err) =
                        queue_chat_message_for_player(player.id, serialized.clone(), redis).await
                    {
                        tracing::error!(
                            "Failed to queue chat message for player {}: {}",
                            player.id,
                            queue_err
                        );
                    }
                    return;
                }
            }
        } else if chat_msg.should_queue() {
            if let Err(e) =
                queue_chat_message_for_player(player.id, serialized.clone(), redis).await
            {
                tracing::error!(
                    "Failed to queue chat message for offline player {}: {}",
                    player.id,
                    e
                );
            }
        }
    }
}
