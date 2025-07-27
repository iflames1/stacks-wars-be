use std::collections::HashMap;

use axum::extract::ws::Message;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use uuid::Uuid;

use crate::{
    db,
    models::{
        chat::{ChatClientMessage, ChatMessage, ChatServerMessage, VoiceParticipant},
        game::Player,
    },
    state::{ChatConnectionInfoMap, ChatHistories, RedisClient, VoiceParticipants, VoiceRooms},
    ws::handlers::chat::utils::{queue_chat_message_for_player, send_chat_message_to_player},
};

pub async fn handle_incoming_chat_messages(
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    room_id: Uuid,
    player: &Player,
    chat_connections: &ChatConnectionInfoMap,
    redis: RedisClient,
    chat_histories: ChatHistories,
    voice_participants: VoiceParticipants,
    _voice_rooms: VoiceRooms,
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
                                handle_text_chat(
                                    &text,
                                    room_id,
                                    player,
                                    chat_connections,
                                    &redis,
                                    &chat_histories,
                                )
                                .await;
                            }
                            ChatClientMessage::Mic { enabled } => {
                                handle_mic_toggle(
                                    enabled,
                                    room_id,
                                    player,
                                    chat_connections,
                                    &redis,
                                    &voice_participants,
                                )
                                .await;
                            }
                            ChatClientMessage::Mute { muted } => {
                                handle_mute_toggle(
                                    muted,
                                    room_id,
                                    player,
                                    chat_connections,
                                    &redis,
                                    &voice_participants,
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

                    // Clean up voice participant when disconnecting
                    {
                        let mut participants = voice_participants.lock().await;
                        if let Some(room_participants) = participants.get_mut(&room_id) {
                            room_participants.remove(&player.id);

                            // Notify other participants about the disconnect
                            if let Some(participant) = room_participants.get(&player.id) {
                                let update_msg = ChatServerMessage::VoiceParticipantUpdate {
                                    participant: participant.clone(),
                                };
                                broadcast_to_voice_participants(
                                    room_id,
                                    &update_msg,
                                    chat_connections,
                                    &redis,
                                    &participants,
                                )
                                .await;
                            }
                        }
                    }
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

async fn handle_text_chat(
    text: &str,
    room_id: Uuid,
    player: &Player,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
    chat_histories: &ChatHistories,
) {
    let room_players = match db::room::get_room_players(room_id, redis.clone()).await {
        Ok(players) => players,
        Err(e) => {
            tracing::error!("Failed to get room players: {}", e);
            let error_msg = ChatServerMessage::Error {
                message: "Failed to verify room membership".to_string(),
            };
            send_chat_message_to_player(player.id, &error_msg, chat_connections, redis).await;
            return;
        }
    };

    let is_room_member = room_players.iter().any(|p| p.id == player.id);

    if !is_room_member {
        let error_msg = ChatServerMessage::Error {
            message: "You are not a member of this room".to_string(),
        };
        send_chat_message_to_player(player.id, &error_msg, chat_connections, redis).await;
        return;
    }

    if text.trim().is_empty() {
        let error_msg = ChatServerMessage::Error {
            message: "Message cannot be empty".to_string(),
        };
        send_chat_message_to_player(player.id, &error_msg, chat_connections, redis).await;
        return;
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

    broadcast_chat_to_room(&chat_message, &room_players, chat_connections, redis).await;
}

async fn handle_mic_toggle(
    enabled: bool,
    room_id: Uuid,
    player: &Player,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
    voice_participants: &VoiceParticipants,
) {
    let mut participants = voice_participants.lock().await;
    let room_participants = participants.entry(room_id).or_insert_with(HashMap::new);

    // Update or create participant
    if let Some(participant) = room_participants.get_mut(&player.id) {
        participant.mic_enabled = enabled;
    } else {
        let new_participant = VoiceParticipant {
            player: player.clone(),
            mic_enabled: enabled,
            is_muted: false,
            is_speaking: false,
        };
        room_participants.insert(player.id, new_participant);
    }

    // Get updated participant for broadcast
    if let Some(participant) = room_participants.get(&player.id) {
        let update_msg = ChatServerMessage::VoiceParticipantUpdate {
            participant: participant.clone(),
        };

        broadcast_to_voice_participants(
            room_id,
            &update_msg,
            chat_connections,
            redis,
            &participants,
        )
        .await;
    }

    tracing::info!(
        "Player {} {} their mic",
        player.wallet_address,
        if enabled { "enabled" } else { "disabled" }
    );
}

async fn handle_mute_toggle(
    muted: bool,
    room_id: Uuid,
    player: &Player,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
    voice_participants: &VoiceParticipants,
) {
    let mut participants = voice_participants.lock().await;
    let room_participants = participants.entry(room_id).or_insert_with(HashMap::new);

    // Update or create participant
    if let Some(participant) = room_participants.get_mut(&player.id) {
        participant.is_muted = muted;
    } else {
        let new_participant = VoiceParticipant {
            player: player.clone(),
            mic_enabled: false, // Default mic off
            is_muted: muted,
            is_speaking: false,
        };
        room_participants.insert(player.id, new_participant);
    }

    // Get updated participant for broadcast
    if let Some(participant) = room_participants.get(&player.id) {
        let update_msg = ChatServerMessage::VoiceParticipantUpdate {
            participant: participant.clone(),
        };

        broadcast_to_voice_participants(
            room_id,
            &update_msg,
            chat_connections,
            redis,
            &participants,
        )
        .await;
    }

    tracing::info!(
        "Player {} {} themselves",
        player.wallet_address,
        if muted { "muted" } else { "unmuted" }
    );
}

async fn broadcast_to_voice_participants(
    room_id: Uuid,
    msg: &ChatServerMessage,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
    participants: &std::collections::HashMap<
        Uuid,
        std::collections::HashMap<Uuid, VoiceParticipant>,
    >,
) {
    if let Some(room_participants) = participants.get(&room_id) {
        let serialized = match serde_json::to_string(msg) {
            Ok(json) => json,
            Err(e) => {
                tracing::error!("Failed to serialize voice message: {}", e);
                return;
            }
        };

        let connection_guard = chat_connections.lock().await;

        for participant in room_participants.values() {
            if let Some(conn_info) = connection_guard.get(&participant.player.id) {
                let mut sender = conn_info.sender.lock().await;
                if let Err(e) = sender.send(Message::Text(serialized.clone().into())).await {
                    tracing::warn!(
                        "Failed to send voice message to player {}: {}",
                        participant.player.id,
                        e
                    );

                    if msg.should_queue() {
                        drop(sender);
                        drop(connection_guard);

                        if let Err(queue_err) = queue_chat_message_for_player(
                            participant.player.id,
                            serialized.clone(),
                            redis,
                        )
                        .await
                        {
                            tracing::error!(
                                "Failed to queue voice message for player {}: {}",
                                participant.player.id,
                                queue_err
                            );
                        }
                        return;
                    }
                }
            } else if msg.should_queue() {
                if let Err(e) =
                    queue_chat_message_for_player(participant.player.id, serialized.clone(), redis)
                        .await
                {
                    tracing::error!(
                        "Failed to queue voice message for offline player {}: {}",
                        participant.player.id,
                        e
                    );
                }
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
