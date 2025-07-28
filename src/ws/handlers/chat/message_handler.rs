use axum::extract::ws::Message;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use mediasoup::prelude::Transport;
use uuid::Uuid;

use crate::{
    db,
    models::{
        chat::{ChatClientMessage, ChatMessage, ChatServerMessage},
        game::Player,
    },
    state::{ChatConnectionInfoMap, ChatHistories, RedisClient},
    ws::handlers::chat::{
        utils::{queue_chat_message_for_player, send_chat_message_to_player},
        voice::{VoiceConnections, broadcast_voice_participant_update},
    },
};

pub async fn handle_incoming_chat_messages(
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    room_id: Uuid,
    player: &Player,
    chat_connections: &ChatConnectionInfoMap,
    redis: RedisClient,
    chat_histories: ChatHistories,
    voice_connections: VoiceConnections,
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
                            ChatClientMessage::VoiceInit { rtp_capabilities } => {
                                handle_voice_init(
                                    player.id,
                                    rtp_capabilities,
                                    &voice_connections,
                                    room_id,
                                )
                                .await;
                            }
                            ChatClientMessage::ConnectProducerTransport { dtls_parameters } => {
                                handle_connect_producer_transport(
                                    player.id,
                                    dtls_parameters,
                                    &voice_connections,
                                    room_id,
                                    chat_connections,
                                    &redis,
                                )
                                .await;
                            }
                            ChatClientMessage::ConnectConsumerTransport { dtls_parameters } => {
                                handle_connect_consumer_transport(
                                    player.id,
                                    dtls_parameters,
                                    &voice_connections,
                                    room_id,
                                    chat_connections,
                                    &redis,
                                )
                                .await;
                            }
                            ChatClientMessage::Produce {
                                kind,
                                rtp_parameters,
                            } => {
                                handle_produce(
                                    player.id,
                                    kind,
                                    rtp_parameters,
                                    &voice_connections,
                                    room_id,
                                    chat_connections,
                                    &redis,
                                )
                                .await;
                            }
                            ChatClientMessage::Consume { producer_id } => {
                                handle_consume(
                                    player.id,
                                    producer_id,
                                    &voice_connections,
                                    room_id,
                                    chat_connections,
                                    &redis,
                                )
                                .await;
                            }
                            ChatClientMessage::ConsumerResume { id } => {
                                handle_consumer_resume(player.id, id, &voice_connections, room_id)
                                    .await;
                            }
                            ChatClientMessage::Mic { enabled } => {
                                handle_mic_toggle(
                                    player.id,
                                    enabled,
                                    &voice_connections,
                                    room_id,
                                    chat_connections,
                                    &redis,
                                )
                                .await;
                            }
                            ChatClientMessage::Mute { muted } => {
                                handle_mute_toggle(
                                    player.id,
                                    muted,
                                    &voice_connections,
                                    room_id,
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
            send_chat_message_to_player(player.id, &error_msg, chat_connections, &redis).await;
            return;
        }
    };

    let is_room_member = room_players.iter().any(|p| p.id == player.id);

    if !is_room_member {
        let error_msg = ChatServerMessage::Error {
            message: "You are not a member of this room".to_string(),
        };
        send_chat_message_to_player(player.id, &error_msg, chat_connections, &redis).await;
        return;
    }

    if text.trim().is_empty() {
        let error_msg = ChatServerMessage::Error {
            message: "Message cannot be empty".to_string(),
        };
        send_chat_message_to_player(player.id, &error_msg, chat_connections, &redis).await;
        return;
    }

    let chat_message = ChatMessage {
        id: Uuid::new_v4(),
        text: text.trim().to_string(),
        sender: player.clone(),
        timestamp: Utc::now(),
    };

    {
        let mut histories = chat_histories.lock().await;
        let history = histories
            .entry(room_id)
            .or_insert_with(|| crate::state::ChatHistory::new());
        history.add_message(chat_message.clone());
    }

    broadcast_chat_to_room(&chat_message, &room_players, chat_connections, &redis).await;
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

// Voice message handlers
async fn handle_voice_init(
    player_id: Uuid,
    rtp_capabilities: mediasoup::rtp_parameters::RtpCapabilities,
    voice_connections: &VoiceConnections,
    room_id: Uuid,
) {
    // Store client RTP capabilities for later use
    let mut connections = voice_connections.lock().await;
    if let Some(room_connections) = connections.get_mut(&room_id) {
        if let Some(voice_conn) = room_connections.get_mut(&player_id) {
            voice_conn.client_rtp_capabilities = Some(rtp_capabilities);
            tracing::info!("Voice initialized for player {}", player_id);
        }
    }
}

async fn handle_connect_producer_transport(
    player_id: Uuid,
    dtls_parameters: mediasoup::data_structures::DtlsParameters,
    voice_connections: &VoiceConnections,
    room_id: Uuid,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let mut connections = voice_connections.lock().await;
    if let Some(room_connections) = connections.get_mut(&room_id) {
        if let Some(voice_conn) = room_connections.get_mut(&player_id) {
            let transport = voice_conn.producer_transport.clone();
            drop(connections);

            match transport
                .connect(
                    mediasoup::webrtc_transport::WebRtcTransportRemoteParameters {
                        dtls_parameters,
                    },
                )
                .await
            {
                Ok(_) => {
                    let msg = ChatServerMessage::ConnectedProducerTransport;
                    send_chat_message_to_player(player_id, &msg, chat_connections, redis).await;
                    tracing::info!("Producer transport connected for player {}", player_id);
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to connect producer transport for player {}: {}",
                        player_id,
                        e
                    );
                    let error_msg = ChatServerMessage::Error {
                        message: "Failed to connect producer transport".to_string(),
                    };
                    send_chat_message_to_player(player_id, &error_msg, chat_connections, redis)
                        .await;
                }
            }
        }
    }
}

async fn handle_connect_consumer_transport(
    player_id: Uuid,
    dtls_parameters: mediasoup::data_structures::DtlsParameters,
    voice_connections: &VoiceConnections,
    room_id: Uuid,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let mut connections = voice_connections.lock().await;
    if let Some(room_connections) = connections.get_mut(&room_id) {
        if let Some(voice_conn) = room_connections.get_mut(&player_id) {
            let transport = voice_conn.consumer_transport.clone();
            drop(connections);

            match transport
                .connect(
                    mediasoup::webrtc_transport::WebRtcTransportRemoteParameters {
                        dtls_parameters,
                    },
                )
                .await
            {
                Ok(_) => {
                    let msg = ChatServerMessage::ConnectedConsumerTransport;
                    send_chat_message_to_player(player_id, &msg, chat_connections, redis).await;
                    tracing::info!("Consumer transport connected for player {}", player_id);
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to connect consumer transport for player {}: {}",
                        player_id,
                        e
                    );
                    let error_msg = ChatServerMessage::Error {
                        message: "Failed to connect consumer transport".to_string(),
                    };
                    send_chat_message_to_player(player_id, &error_msg, chat_connections, redis)
                        .await;
                }
            }
        }
    }
}

async fn handle_produce(
    player_id: Uuid,
    kind: mediasoup::rtp_parameters::MediaKind,
    rtp_parameters: mediasoup::rtp_parameters::RtpParameters,
    voice_connections: &VoiceConnections,
    room_id: Uuid,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let mut connections = voice_connections.lock().await;
    if let Some(room_connections) = connections.get_mut(&room_id) {
        if let Some(voice_conn) = room_connections.get_mut(&player_id) {
            let transport = voice_conn.producer_transport.clone();
            drop(connections);

            match transport
                .produce(mediasoup::producer::ProducerOptions::new(
                    kind,
                    rtp_parameters,
                ))
                .await
            {
                Ok(producer) => {
                    let producer_id = producer.id();

                    // Store producer
                    let mut connections = voice_connections.lock().await;
                    if let Some(room_connections) = connections.get_mut(&room_id) {
                        if let Some(voice_conn) = room_connections.get_mut(&player_id) {
                            voice_conn.producers.push(producer);
                        }
                    }
                    drop(connections);

                    let msg = ChatServerMessage::Produced { id: producer_id };
                    send_chat_message_to_player(player_id, &msg, chat_connections, redis).await;
                    tracing::info!(
                        "{:?} producer created for player {}: {}",
                        kind,
                        player_id,
                        producer_id
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to create {:?} producer for player {}: {}",
                        kind,
                        player_id,
                        e
                    );
                    let error_msg = ChatServerMessage::Error {
                        message: format!("Failed to create {:?} producer", kind),
                    };
                    send_chat_message_to_player(player_id, &error_msg, chat_connections, redis)
                        .await;
                }
            }
        }
    }
}

async fn handle_consume(
    player_id: Uuid,
    producer_id: mediasoup::producer::ProducerId,
    voice_connections: &VoiceConnections,
    room_id: Uuid,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let connections = voice_connections.lock().await;
    if let Some(room_connections) = connections.get(&room_id) {
        if let Some(voice_conn) = room_connections.get(&player_id) {
            let transport = voice_conn.consumer_transport.clone();
            let client_rtp_capabilities = voice_conn.client_rtp_capabilities.clone();
            drop(connections);

            if let Some(rtp_capabilities) = client_rtp_capabilities {
                let mut options =
                    mediasoup::consumer::ConsumerOptions::new(producer_id, rtp_capabilities);
                options.paused = true;

                match transport.consume(options).await {
                    Ok(consumer) => {
                        let consumer_id = consumer.id();
                        let kind = consumer.kind();
                        let rtp_parameters = consumer.rtp_parameters().clone();

                        // Store consumer
                        let mut connections = voice_connections.lock().await;
                        if let Some(room_connections) = connections.get_mut(&room_id) {
                            if let Some(voice_conn) = room_connections.get_mut(&player_id) {
                                voice_conn.consumers.insert(consumer_id, consumer);
                            }
                        }
                        drop(connections);

                        let msg = ChatServerMessage::Consumed {
                            id: consumer_id,
                            producer_id,
                            kind,
                            rtp_parameters,
                        };
                        send_chat_message_to_player(player_id, &msg, chat_connections, redis).await;
                        tracing::info!(
                            "{:?} consumer created for player {}: {}",
                            kind,
                            player_id,
                            consumer_id
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to create consumer for player {}: {}",
                            player_id,
                            e
                        );
                        let error_msg = ChatServerMessage::Error {
                            message: "Failed to create consumer".to_string(),
                        };
                        send_chat_message_to_player(player_id, &error_msg, chat_connections, redis)
                            .await;
                    }
                }
            } else {
                tracing::error!("No client RTP capabilities stored for player {}", player_id);
                let error_msg = ChatServerMessage::Error {
                    message: "Voice not initialized".to_string(),
                };
                send_chat_message_to_player(player_id, &error_msg, chat_connections, redis).await;
            }
        }
    }
}
async fn handle_consumer_resume(
    player_id: Uuid,
    consumer_id: mediasoup::consumer::ConsumerId,
    voice_connections: &VoiceConnections,
    room_id: Uuid,
) {
    let connections = voice_connections.lock().await;
    if let Some(room_connections) = connections.get(&room_id) {
        if let Some(voice_conn) = room_connections.get(&player_id) {
            if let Some(consumer) = voice_conn.consumers.get(&consumer_id) {
                let consumer = consumer.clone();
                drop(connections);

                if let Err(e) = consumer.resume().await {
                    tracing::error!(
                        "Failed to resume consumer {} for player {}: {}",
                        consumer_id,
                        player_id,
                        e
                    );
                } else {
                    tracing::info!("Consumer {} resumed for player {}", consumer_id, player_id);
                }
            }
        }
    }
}

async fn handle_mic_toggle(
    player_id: Uuid,
    enabled: bool,
    voice_connections: &VoiceConnections,
    room_id: Uuid,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let mut connections = voice_connections.lock().await;
    if let Some(room_connections) = connections.get_mut(&room_id) {
        if let Some(voice_conn) = room_connections.get_mut(&player_id) {
            voice_conn.participant.mic_enabled = enabled;
            let participant = voice_conn.participant.clone();
            drop(connections);

            if let Ok(room_players) = db::room::get_room_players(room_id, redis.clone()).await {
                broadcast_voice_participant_update(
                    room_id,
                    participant,
                    &room_players,
                    chat_connections,
                    redis,
                )
                .await;
            }

            tracing::info!(
                "Player {} mic {}",
                player_id,
                if enabled { "enabled" } else { "disabled" }
            );
        }
    }
}

async fn handle_mute_toggle(
    player_id: Uuid,
    muted: bool,
    voice_connections: &VoiceConnections,
    room_id: Uuid,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let mut connections = voice_connections.lock().await;
    if let Some(room_connections) = connections.get_mut(&room_id) {
        if let Some(voice_conn) = room_connections.get_mut(&player_id) {
            voice_conn.participant.is_muted = muted;
            let participant = voice_conn.participant.clone();
            drop(connections);

            if let Ok(room_players) = db::room::get_room_players(room_id, redis.clone()).await {
                broadcast_voice_participant_update(
                    room_id,
                    participant,
                    &room_players,
                    chat_connections,
                    redis,
                )
                .await;
            }

            tracing::info!(
                "Player {} {}",
                player_id,
                if muted { "muted" } else { "unmuted" }
            );
        }
    }
}
