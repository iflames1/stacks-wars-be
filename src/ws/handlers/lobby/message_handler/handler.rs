use axum::extract::ws::Message;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use uuid::Uuid;

use crate::{
    db,
    errors::AppError,
    models::{
        User,
        game::Player,
        lobby::{JoinRequest, JoinState, LobbyClientMessage, LobbyServerMessage, PendingJoin},
    },
    state::{ConnectionInfoMap, LobbyCountdowns, LobbyJoinRequests, RedisClient},
    ws::handlers::{
        lobby::message_handler::{
            join_lobby::join_lobby, kick_player, leave_room, permit_join, ping, request_join,
            update_game_state, update_player_state,
        },
        utils::queue_message_for_player,
    },
};

pub async fn broadcast_to_lobby(
    room_id: Uuid,
    msg: &LobbyServerMessage,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    let serialized = match serde_json::to_string(msg) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize message: {}", e);
            return;
        }
    };

    if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
        let connection_guard = connections.lock().await;

        for player in &players {
            if let Some(conn_info) = connection_guard.get(&player.id) {
                // Try to send immediately
                let mut sender = conn_info.sender.lock().await;
                if let Err(e) = sender.send(Message::Text(serialized.clone().into())).await {
                    tracing::warn!("Failed to send message to player {}: {}", player.id, e);

                    // Only queue the message if it should be queued
                    if msg.should_queue() {
                        drop(sender); // Release the sender lock
                        drop(connection_guard); // Release the connection guard

                        if let Err(queue_err) =
                            queue_message_for_player(player.id, serialized.clone(), &redis).await
                        {
                            tracing::error!(
                                "Failed to queue message for player {}: {}",
                                player.id,
                                queue_err
                            );
                        }
                    }

                    return; // Exit early to avoid double-locking
                }
            } else {
                // Player not connected, only queue if message should be queued
                if msg.should_queue() {
                    if let Err(e) =
                        queue_message_for_player(player.id, serialized.clone(), &redis).await
                    {
                        tracing::error!(
                            "Failed to queue message for offline player {}: {}",
                            player.id,
                            e
                        );
                    }
                }
            }
        }
    }
}

pub async fn send_error_to_player(
    player_id: Uuid,
    message: impl Into<String>,
    connection_info: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let error_msg = LobbyServerMessage::Error {
        message: message.into(),
    };
    send_to_player(player_id, connection_info, &error_msg, redis).await;
}

pub async fn send_to_player(
    player_id: Uuid,
    connection_info: &ConnectionInfoMap,
    msg: &LobbyServerMessage,
    redis: &RedisClient,
) {
    let serialized = match serde_json::to_string(msg) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize message: {}", e);
            return;
        }
    };

    let conns = connection_info.lock().await;
    if let Some(conn_info) = conns.get(&player_id) {
        let mut sender = conn_info.sender.lock().await;
        if let Err(e) = sender.send(Message::Text(serialized.clone().into())).await {
            tracing::error!("Failed to send message to player {}: {}", player_id, e);

            // Only queue the message if it should be queued
            if msg.should_queue() {
                drop(sender);
                drop(conns);

                if let Err(queue_err) = queue_message_for_player(player_id, serialized, redis).await
                {
                    tracing::error!(
                        "Failed to queue message for player {}: {}",
                        player_id,
                        queue_err
                    );
                }
            }
        }
    } else {
        // Player not connected, only queue if message should be queued
        if msg.should_queue() {
            if let Err(e) = queue_message_for_player(player_id, serialized, redis).await {
                tracing::error!(
                    "Failed to queue message for offline player {}: {}",
                    player_id,
                    e
                );
            }
        }
    }
}

pub async fn get_join_requests(
    room_id: Uuid,
    join_requests: &LobbyJoinRequests,
) -> Vec<JoinRequest> {
    let map = join_requests.lock().await;
    map.get(&room_id).cloned().unwrap_or_default()
}

pub async fn request_to_join(
    room_id: Uuid,
    user: User,
    join_requests: &LobbyJoinRequests,
) -> Result<(), AppError> {
    let mut map = join_requests.lock().await;
    let room_requests = map.entry(room_id).or_default();

    if let Some(existing) = room_requests.iter_mut().find(|r| r.user.id == user.id) {
        if existing.state == JoinState::Pending {
            return Ok(());
        }

        existing.state = JoinState::Pending;
        return Ok(());
    }

    room_requests.push(JoinRequest {
        user,
        state: JoinState::Pending,
    });

    Ok(())
}

pub async fn accept_join_request(
    room_id: Uuid,
    user_id: Uuid,
    join_requests: &LobbyJoinRequests,
) -> Result<(), AppError> {
    let mut map = join_requests.lock().await;

    if let Some(requests) = map.get_mut(&room_id) {
        tracing::info!("Join requests for room {}: {:?}", room_id, requests);

        if let Some(req) = requests.iter_mut().find(|r| r.user.id == user_id) {
            tracing::info!("Found join request for user {}", user_id);
            req.state = JoinState::Allowed;
            return Ok(());
        } else {
            tracing::warn!("User {} not found in join requests", user_id);
        }
    } else {
        tracing::warn!("No join requests found for room {}", room_id);
    }

    Err(AppError::NotFound("User not found in join requests".into()))
}

pub async fn reject_join_request(
    room_id: Uuid,
    user_id: Uuid,
    join_requests: &LobbyJoinRequests,
) -> Result<(), AppError> {
    let mut map = join_requests.lock().await;

    if let Some(requests) = map.get_mut(&room_id) {
        tracing::info!("Join requests for room {}: {:?}", room_id, requests);

        if let Some(req) = requests.iter_mut().find(|r| r.user.id == user_id) {
            tracing::info!("Found join request for user {}", user_id);
            req.state = JoinState::Rejected;
            return Ok(());
        } else {
            tracing::warn!("User {} not found in join requests", user_id);
        }
    } else {
        tracing::warn!("No join requests found for room {}", room_id);
    }

    Err(AppError::NotFound("User not found in join requests".into()))
}

pub async fn get_pending_players(
    room_id: Uuid,
    join_requests: &LobbyJoinRequests,
) -> Result<Vec<PendingJoin>, AppError> {
    let map = get_join_requests(room_id, join_requests).await;
    let pending_players = map
        .into_iter()
        .filter(|req| req.state == JoinState::Pending)
        .map(|req| PendingJoin {
            user: req.user,
            state: req.state,
        })
        .collect();

    Ok(pending_players)
}

pub async fn handle_incoming_messages(
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    room_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
    join_requests: LobbyJoinRequests,
    countdowns: LobbyCountdowns,
) {
    while let Some(msg_result) = receiver.next().await {
        match msg_result {
            Ok(msg) => match msg {
                Message::Text(text) => {
                    if let Ok(parsed) = serde_json::from_str::<LobbyClientMessage>(&text) {
                        match parsed {
                            LobbyClientMessage::Ping { ts } => {
                                ping(ts, player, connections, &redis).await
                            }
                            LobbyClientMessage::JoinLobby { tx_id } => {
                                join_lobby(
                                    tx_id,
                                    room_id,
                                    &join_requests,
                                    player,
                                    connections,
                                    &redis,
                                )
                                .await
                            }
                            LobbyClientMessage::RequestJoin => {
                                request_join(player, room_id, &join_requests, connections, &redis)
                                    .await
                            }
                            LobbyClientMessage::PermitJoin { user_id, allow } => {
                                permit_join(
                                    allow,
                                    user_id,
                                    &join_requests,
                                    player.clone(),
                                    room_id,
                                    connections,
                                    &redis,
                                )
                                .await
                            }
                            LobbyClientMessage::UpdatePlayerState { new_state } => {
                                update_player_state(new_state, room_id, player, connections, &redis)
                                    .await
                            }
                            LobbyClientMessage::LeaveRoom => {
                                leave_room(room_id, player, connections, &redis).await
                            }
                            LobbyClientMessage::KickPlayer {
                                player_id,
                                wallet_address,
                                display_name,
                            } => {
                                kick_player(
                                    player_id,
                                    wallet_address,
                                    display_name,
                                    room_id,
                                    player,
                                    connections,
                                    &redis,
                                )
                                .await
                            }
                            LobbyClientMessage::UpdateGameState { new_state } => {
                                update_game_state(
                                    new_state,
                                    room_id,
                                    player,
                                    connections,
                                    &redis,
                                    &countdowns,
                                )
                                .await
                            }
                        }
                    }
                }
                Message::Ping(data) => {
                    // Handle WebSocket ping with custom timestamp logic
                    if data.len() == 8 {
                        // Extract timestamp from ping data (8 bytes for u64)
                        let mut ts_bytes = [0u8; 8];
                        if data.len() >= 8 {
                            ts_bytes.copy_from_slice(&data[..8]);
                        }
                        let client_ts = u64::from_le_bytes(ts_bytes);

                        let now = Utc::now().timestamp_millis() as u64;
                        let pong_time = now.saturating_sub(client_ts);

                        // Send as LobbyServerMessage::Pong JSON instead of WebSocket pong
                        let pong_msg = LobbyServerMessage::Pong {
                            ts: client_ts,
                            pong: pong_time,
                        };
                        send_to_player(player.id, &connections, &pong_msg, &redis).await;
                    } else {
                        // For standard WebSocket pings without timestamp, use current time
                        let now = Utc::now().timestamp_millis() as u64;
                        let pong_msg = LobbyServerMessage::Pong { ts: now, pong: 0 };
                        send_to_player(player.id, &connections, &pong_msg, &redis).await;
                    }
                }
                Message::Pong(_) => {
                    // Client responded to our ping (if we were sending any)
                    tracing::debug!("Received pong from player {}", player.id);
                }
                Message::Close(_) => {
                    tracing::info!("Player {} closed connection", player.wallet_address);
                    break;
                }
                _ => {}
            },
            Err(e) => {
                tracing::error!(
                    "WebSocket error for player {}: {}",
                    player.wallet_address,
                    e
                );
                break;
            }
        }
    }
}
