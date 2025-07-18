use axum::extract::ws::Message;
use futures::{SinkExt, StreamExt};
use uuid::Uuid;

use crate::{
    db,
    errors::AppError,
    models::{
        User,
        game::{GameState, LobbyClientMessage, LobbyServerMessage, PendingJoin, Player},
        lobby::{JoinRequest, JoinState},
    },
    state::{LobbyJoinRequests, PlayerConnections, RedisClient},
    ws::handlers::remove_connection,
};

pub async fn broadcast_to_lobby(
    room_id: Uuid,
    msg: &LobbyServerMessage,
    connections: &PlayerConnections,
    redis: RedisClient,
) {
    let serialized = match serde_json::to_string(msg) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize message: {}", e);
            return;
        }
    };

    if let Ok(players) = db::room::get_room_players(room_id, redis).await {
        let connection_guard = connections.lock().await;

        for player in players {
            if let Some(sender_arc) = connection_guard.get(&player.id) {
                let mut sender = sender_arc.lock().await;
                let _ = sender.send(Message::Text(serialized.clone().into())).await;
            }
        }
    }
}

async fn send_error_to_player(
    player_id: Uuid,
    message: impl Into<String>,
    connections: &PlayerConnections,
) {
    let conns = connections.lock().await;
    if let Some(sender) = conns.get(&player_id) {
        let mut sender = sender.lock().await;
        let error_msg = LobbyServerMessage::Error {
            message: message.into(),
        };
        if let Ok(text) = serde_json::to_string(&error_msg) {
            let _ = sender.send(Message::Text(text.into())).await;
        }
    }
}

async fn send_to_player(
    player_id: Uuid,
    connections: &PlayerConnections,
    msg: &LobbyServerMessage,
) {
    let conns = connections.lock().await;
    if let Some(sender) = conns.get(&player_id) {
        let mut sender = sender.lock().await;
        let _ = sender
            .send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
            .await;
        let _ = sender.send(Message::Close(None)).await;
    }
}

async fn get_join_requests(room_id: Uuid, join_requests: &LobbyJoinRequests) -> Vec<JoinRequest> {
    let map = join_requests.lock().await;
    map.get(&room_id).cloned().unwrap_or_default()
}

async fn request_to_join(
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

async fn accept_join_request(
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

async fn reject_join_request(
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

async fn get_pending_players(
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
    connections: &PlayerConnections,
    redis: RedisClient,
    join_requests: LobbyJoinRequests,
) {
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(parsed) = serde_json::from_str::<LobbyClientMessage>(&text) {
                    match parsed {
                        LobbyClientMessage::JoinLobby { tx_id } => {
                            let join_map = get_join_requests(room_id, &join_requests).await;
                            if let Some(req) = join_map.iter().find(|r| r.user.id == player.id) {
                                if req.state == JoinState::Allowed {
                                    if let Err(e) = db::room::join_room(
                                        room_id,
                                        player.id,
                                        tx_id,
                                        redis.clone(),
                                    )
                                    .await
                                    {
                                        tracing::error!("Failed to join room: {}", e);
                                        send_error_to_player(
                                            player.id,
                                            e.to_string(),
                                            &connections,
                                        )
                                        .await;
                                    } else if let Ok(players) =
                                        db::room::get_room_players(room_id, redis.clone()).await
                                    {
                                        tracing::info!(
                                            "{} joined room {} successfully",
                                            player.wallet_address,
                                            room_id
                                        );
                                        let msg = LobbyServerMessage::PlayerJoined { players };
                                        broadcast_to_lobby(
                                            room_id,
                                            &msg,
                                            &connections,
                                            redis.clone(),
                                        )
                                        .await;
                                    }
                                } else {
                                    tracing::warn!(
                                        "User {} attempted to join without being allowed",
                                        player.wallet_address
                                    );
                                    send_error_to_player(
                                        player.id,
                                        "Join request has to be accpeted to join lobby",
                                        &connections,
                                    )
                                    .await;
                                }
                            }
                        }
                        LobbyClientMessage::RequestJoin => {
                            match request_to_join(room_id, player.clone().into(), &join_requests)
                                .await
                            {
                                Ok(_) => {
                                    if let Ok(pending_players) =
                                        get_pending_players(room_id, &join_requests).await
                                    {
                                        tracing::info!(
                                            "Success Adding {} to pending players",
                                            player.wallet_address
                                        );
                                        let msg = LobbyServerMessage::Pending;
                                        send_to_player(player.id, &connections, &msg).await;

                                        let msg =
                                            LobbyServerMessage::PendingPlayers { pending_players };
                                        broadcast_to_lobby(
                                            room_id,
                                            &msg,
                                            &connections,
                                            redis.clone(),
                                        )
                                        .await;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to mark user as pending: {}", e);
                                    send_error_to_player(
                                        player.id,
                                        "Failed to send join request",
                                        &connections,
                                    )
                                    .await;
                                }
                            }
                        }
                        LobbyClientMessage::PermitJoin { user_id, allow } => {
                            let room_info = match db::room::get_room_info(room_id, redis.clone())
                                .await
                            {
                                Ok(info) => info,
                                Err(e) => {
                                    tracing::error!("Failed to fetch room info: {}", e);
                                    send_error_to_player(player.id, e.to_string(), &connections)
                                        .await;
                                    continue;
                                }
                            };

                            if room_info.creator_id != player.id {
                                tracing::warn!(
                                    "Unauthorized PermitJoin attempt by {}",
                                    player.wallet_address
                                );
                                send_error_to_player(
                                    player.id,
                                    "Only creator can accept request",
                                    &connections,
                                )
                                .await;
                                continue;
                            }

                            let result = if allow {
                                accept_join_request(room_id, user_id, &join_requests).await
                            } else {
                                reject_join_request(room_id, user_id, &join_requests).await
                            };

                            match result {
                                Ok(_) => {
                                    let msg = if allow {
                                        LobbyServerMessage::Allowed
                                    } else {
                                        LobbyServerMessage::Rejected
                                    };
                                    send_to_player(user_id, &connections, &msg).await;
                                }
                                Err(e) => {
                                    tracing::error!("Failed to update join state: {}", e);
                                    send_error_to_player(user_id, e.to_string(), &connections)
                                        .await;
                                }
                            }

                            if let Ok(pending_players) =
                                get_pending_players(room_id, &join_requests).await
                            {
                                tracing::info!(
                                    "Updated pending players for room {}: {}",
                                    room_id,
                                    pending_players.len()
                                );
                                let msg = LobbyServerMessage::PendingPlayers { pending_players };
                                broadcast_to_lobby(room_id, &msg, &connections, redis.clone())
                                    .await;
                            }
                        }
                        LobbyClientMessage::UpdatePlayerState { new_state } => {
                            if let Err(e) = db::room::update_player_state(
                                room_id,
                                player.id,
                                new_state.clone(),
                                redis.clone(),
                            )
                            .await
                            {
                                tracing::error!("Failed to update state: {}", e);
                                send_error_to_player(player.id, e.to_string(), &connections).await;
                            } else if let Ok(players) =
                                db::room::get_room_players(room_id, redis.clone()).await
                            {
                                tracing::info!(
                                    "Player {} updated state to {:?} in room {}",
                                    player.wallet_address,
                                    new_state,
                                    room_id
                                );
                                let msg = LobbyServerMessage::PlayerUpdated { players };
                                broadcast_to_lobby(room_id, &msg, &connections, redis.clone())
                                    .await;
                            }
                        }
                        LobbyClientMessage::LeaveRoom => {
                            if let Err(e) =
                                db::room::leave_room(room_id, player.id, redis.clone()).await
                            {
                                tracing::error!("Failed to leave room: {}", e);
                                send_error_to_player(player.id, e.to_string(), &connections).await;
                            } else if let Ok(players) =
                                db::room::get_room_players(room_id, redis.clone()).await
                            {
                                tracing::info!(
                                    "Player {} left room {}",
                                    player.wallet_address,
                                    room_id
                                );
                                let msg = LobbyServerMessage::PlayerLeft { players };
                                broadcast_to_lobby(room_id, &msg, &connections, redis.clone())
                                    .await;
                            }
                            remove_connection(player.id, &connections).await;
                            break;
                        }
                        LobbyClientMessage::KickPlayer {
                            player_id,
                            wallet_address,
                            display_name,
                        } => {
                            let room_info = match db::room::get_room_info(room_id, redis.clone())
                                .await
                            {
                                Ok(info) => info,
                                Err(e) => {
                                    tracing::error!("Failed to fetch room info: {}", e);
                                    send_error_to_player(player.id, e.to_string(), &connections)
                                        .await;
                                    continue;
                                }
                            };

                            if room_info.creator_id != player.id {
                                tracing::error!(
                                    "Unauthorized kick attempt by {}",
                                    player.wallet_address
                                );
                                send_error_to_player(
                                    player.id,
                                    "Only creator can kick players",
                                    &connections,
                                )
                                .await;
                                continue;
                            }

                            if room_info.state != GameState::Waiting {
                                tracing::error!("Cannot kick players when game is not waiting");
                                send_error_to_player(
                                    player.id,
                                    "Cannot kick player when game is in progress",
                                    &connections,
                                )
                                .await;
                                continue;
                            }

                            // Remove player
                            if let Err(e) =
                                db::room::leave_room(room_id, player_id, redis.clone()).await
                            {
                                tracing::error!("Failed to kick player: {}", e);
                                send_error_to_player(player.id, e.to_string(), &connections).await;
                            } else if let Ok(players) =
                                db::room::get_room_players(room_id, redis.clone()).await
                            {
                                let msg = LobbyServerMessage::PlayerLeft { players };
                                broadcast_to_lobby(room_id, &msg, &connections, redis.clone())
                                    .await;

                                tracing::info!(
                                    "Success kicking {} from {}",
                                    player.wallet_address,
                                    room_id
                                );
                                let kicked_msg = LobbyServerMessage::PlayerKicked {
                                    player_id,
                                    wallet_address,
                                    display_name,
                                };
                                broadcast_to_lobby(
                                    room_id,
                                    &kicked_msg,
                                    &connections,
                                    redis.clone(),
                                )
                                .await;

                                let msg: LobbyServerMessage = LobbyServerMessage::NotifyKicked;

                                send_to_player(player_id, &connections, &msg).await;
                            }

                            // Optionally: disconnect the kicked player
                            let conns = connections.lock().await;
                            if let Some(sender) = conns.get(&player_id) {
                                let mut sender = sender.lock().await;
                                let _ = sender.send(Message::Close(None)).await;
                            }
                        }
                        LobbyClientMessage::UpdateGameState { new_state } => {
                            let room_info = match db::room::get_room_info(room_id, redis.clone())
                                .await
                            {
                                Ok(info) => info,
                                Err(e) => {
                                    tracing::error!("Failed to fetch room info: {}", e);
                                    send_error_to_player(player.id, e.to_string(), &connections)
                                        .await;
                                    continue;
                                }
                            };

                            if room_info.creator_id != player.id {
                                tracing::warn!(
                                    "Unauthorized game state update attempt by {}",
                                    player.wallet_address
                                );
                                send_error_to_player(
                                    player.id,
                                    "Only creator can update game state",
                                    &connections,
                                )
                                .await;
                                continue;
                            }

                            if let Err(e) = db::room::update_game_state(
                                room_id,
                                new_state.clone(),
                                redis.clone(),
                            )
                            .await
                            {
                                tracing::error!("Failed to update game state: {}", e);
                                send_error_to_player(player.id, e.to_string(), &connections).await;
                            } else {
                                if new_state == GameState::InProgress {
                                    let redis_clone = redis.clone();
                                    let conns_clone = connections.clone();
                                    let player_clone = player.clone();
                                    tokio::spawn(async move {
                                        start_countdown(
                                            room_id,
                                            player_clone,
                                            redis_clone,
                                            conns_clone,
                                        )
                                        .await;
                                    });

                                    if let Ok(info) =
                                        db::room::get_room_info(room_id, redis.clone()).await
                                    {
                                        if info.state == GameState::InProgress {
                                            let game_starting = LobbyServerMessage::GameState {
                                                state: new_state,
                                                ready_players: None,
                                            };
                                            broadcast_to_lobby(
                                                room_id,
                                                &game_starting,
                                                &connections,
                                                redis.clone(),
                                            )
                                            .await;
                                        } else {
                                            tracing::info!(
                                                "Game state was reverted before start, skipping GameState message"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

async fn start_countdown(
    room_id: Uuid,
    player: Player,
    redis: RedisClient,
    connections: PlayerConnections,
) {
    for i in (0..=30).rev() {
        match db::room::get_room_info(room_id, redis.clone()).await {
            Ok(info) => {
                if info.state != GameState::InProgress {
                    tracing::info!("Countdown interrupted by state change");

                    let msg = LobbyServerMessage::GameState {
                        state: info.state,
                        ready_players: None,
                    };
                    broadcast_to_lobby(room_id, &msg, &connections, redis.clone()).await;

                    break;
                }
            }
            Err(e) => {
                tracing::error!("Failed to check state: {}", e);
                send_error_to_player(player.id, e.to_string(), &connections).await;
                break;
            }
        }

        let countdown_msg = LobbyServerMessage::Countdown { time: i };
        broadcast_to_lobby(room_id, &countdown_msg, &connections, redis.clone()).await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    // Final state confirmation
    if let Ok(info) = db::room::get_room_info(room_id, redis.clone()).await {
        if info.state == GameState::InProgress {
            let ready_players = match db::room::get_ready_room_players(room_id, redis.clone()).await
            {
                Ok(players) => players.into_iter().map(|p| p.id).collect::<Vec<_>>(),
                Err(e) => {
                    tracing::error!("❌ Failed to get ready players: {}", e);
                    send_error_to_player(player.id, e.to_string(), &connections).await;
                    vec![]
                }
            };

            let players = match db::room::get_room_players(room_id, redis.clone()).await {
                Ok(players) => players,
                Err(e) => {
                    tracing::error!("❌ Failed to get room players: {}", e);
                    send_error_to_player(player.id, e.to_string(), &connections).await;
                    vec![]
                }
            };

            tracing::info!("Game started with {} ready players", ready_players.len());

            let msg = LobbyServerMessage::GameState {
                state: GameState::InProgress,
                ready_players: Some(ready_players.clone()),
            };
            broadcast_to_lobby(room_id, &msg, &connections, redis.clone()).await;

            if ready_players.len() > 1 {
                for player in players {
                    remove_connection(player.id, &connections).await;
                }
            }
        }
    }
}
