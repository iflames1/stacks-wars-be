use axum::{
    extract::{ConnectInfo, Path, Query, State, WebSocketUpgrade, ws::WebSocket},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;

use crate::ws::handlers::{
    lobby::message_handler::handler::send_error_to_player,
    utils::store_connection_and_send_queued_messages,
};
use crate::{
    db::{
        lobby::{
            countdown::get_lobby_countdown,
            get::{get_lobby_info, get_lobby_players},
            join_requests::get_player_join_request,
        },
        user::get::get_user_by_id,
    },
    models::{
        game::{LobbyState, Player, PlayerState, WsQueryParams},
        lobby::{JoinState, LobbyServerMessage},
    },
    state::{AppState, ChatConnectionInfoMap, RedisClient},
    ws::handlers::lobby::message_handler::handler::{self, get_pending_players},
};
use crate::{state::ConnectionInfoMap, ws::handlers::utils::remove_connection};
use axum::extract::ws::{CloseFrame, Message};
use uuid::Uuid;

pub async fn lobby_ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    Path(lobby_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, String)> {
    tracing::info!("New lobby WS connection from {}", addr);

    let player_id = query.user_id;
    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let chat_connections = state.chat_connections.clone();

    let players = get_lobby_players(lobby_id, None, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    if let Some(matched_player) = players.iter().find(|p| p.id == player_id).cloned() {
        return Ok(ws.on_upgrade(move |socket| {
            handle_lobby_socket(
                socket,
                lobby_id,
                matched_player.into(),
                connections,
                chat_connections,
                redis,
            )
        }));
    }

    let user = get_user_by_id(player_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    let idle_player = Player {
        id: user.id,
        state: PlayerState::NotReady,
        rank: None,
        used_words: None,
        tx_id: None,
        claim: None,
        prize: None,
        last_ping: None,
        user: Some(user.clone()),
    };

    Ok(ws.on_upgrade(move |socket| {
        handle_lobby_socket(
            socket,
            lobby_id,
            idle_player,
            connections,
            chat_connections,
            redis,
        )
    }))
}

async fn handle_lobby_socket(
    socket: WebSocket,
    lobby_id: Uuid,
    player: Player,
    connections: ConnectionInfoMap,
    chat_connections: ChatConnectionInfoMap,
    redis: RedisClient,
) {
    let (mut sender, receiver) = socket.split();

    // Check lobby state immediately upon connection
    match get_lobby_info(lobby_id, redis.clone()).await {
        Ok(lobby_info) => {
            // Check if there's an active countdown from Redis
            let countdown_time = get_lobby_countdown(lobby_id, redis.clone())
                .await
                .unwrap_or(None)
                .unwrap_or_else(|| {
                    if lobby_info.state == LobbyState::InProgress {
                        0
                    } else {
                        15
                    }
                });

            // Always broadcast the current game state
            let ready_players =
                match get_lobby_players(lobby_id, Some(PlayerState::Ready), redis.clone()).await {
                    Ok(players) => players.into_iter().map(|p| p.id).collect::<Vec<_>>(),
                    Err(e) => {
                        tracing::error!("❌ Failed to get ready players: {}", e);
                        send_error_to_player(
                            player.id,
                            lobby_id,
                            e.to_string(),
                            &connections,
                            &redis,
                        )
                        .await;
                        vec![]
                    }
                };
            let game_state_msg = LobbyServerMessage::LobbyState {
                state: lobby_info.state.clone(),
                ready_players: Some(ready_players),
            };

            let serialized = match serde_json::to_string(&game_state_msg) {
                Ok(json) => json,
                Err(e) => {
                    tracing::error!("Failed to serialize LobbyState message: {}", e);
                    return;
                }
            };

            if let Err(e) = sender.send(Message::Text(serialized.into())).await {
                tracing::error!("Failed to send game state to player {}: {}", player.id, e);
                return;
            }

            let countdown_msg = LobbyServerMessage::Countdown {
                time: countdown_time,
            };
            let serialized = match serde_json::to_string(&countdown_msg) {
                Ok(json) => json,
                Err(e) => {
                    tracing::error!("Failed to serialize Countdown message: {}", e);
                    return;
                }
            };

            if let Err(e) = sender.send(Message::Text(serialized.into())).await {
                tracing::error!("Failed to send countdown to player {}: {}", player.id, e);
                return;
            }

            // Check if player has a join request and broadcast their status
            match get_player_join_request(lobby_id, player.id, redis.clone()).await {
                Ok(Some(join_request)) => {
                    // Player has a join request - send their current state
                    let join_state_msg = match join_request.state {
                        JoinState::Pending => LobbyServerMessage::Pending,
                        JoinState::Allowed => LobbyServerMessage::Allowed,
                        JoinState::Rejected => LobbyServerMessage::Rejected,
                    };

                    let serialized = match serde_json::to_string(&join_state_msg) {
                        Ok(json) => json,
                        Err(e) => {
                            tracing::error!("Failed to serialize join state message: {}", e);
                            return;
                        }
                    };

                    if let Err(e) = sender.send(Message::Text(serialized.into())).await {
                        tracing::error!("Failed to send join state to player {}: {}", player.id, e);
                        return;
                    }

                    tracing::info!(
                        "Sent join state {:?} to player {} in lobby {}",
                        join_request.state,
                        player.id,
                        lobby_id
                    );
                }
                Ok(None) => {
                    // No join request found - player might be already in the lobby or never requested
                    tracing::debug!(
                        "No join request found for player {} in lobby {}",
                        player.id,
                        lobby_id
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to get join request for player {} in lobby {}: {}",
                        player.id,
                        lobby_id,
                        e
                    );
                }
            }

            match get_pending_players(lobby_id, redis.clone()).await {
                Ok(pending_players) => {
                    if !pending_players.is_empty() {
                        let pending_count = pending_players.len();
                        let pending_msg = LobbyServerMessage::PendingPlayers { pending_players };
                        let serialized = match serde_json::to_string(&pending_msg) {
                            Ok(json) => json,
                            Err(e) => {
                                tracing::error!(
                                    "Failed to serialize pending players message: {}",
                                    e
                                );
                                return;
                            }
                        };

                        if let Err(e) = sender.send(Message::Text(serialized.into())).await {
                            tracing::error!(
                                "Failed to send pending players to player {}: {}",
                                player.id,
                                e
                            );
                            return;
                        }

                        if lobby_info.creator.id == player.id {
                            tracing::info!(
                                "Sent {} pending players to lobby creator {} in lobby {}",
                                pending_count,
                                player.id,
                                lobby_id
                            );
                        } else {
                            tracing::info!(
                                "Sent {} pending players to player {} in lobby {}",
                                pending_count,
                                player.id,
                                lobby_id
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to get pending players for lobby {}: {}",
                        lobby_id,
                        e
                    );
                }
            }

            // If game is in progress and countdown is 0, close the connection immediately
            if lobby_info.state == LobbyState::InProgress && countdown_time == 0 {
                tracing::info!(
                    "Player {} trying to connect to lobby while game is in progress (countdown finished) - closing connection",
                    player.id
                );

                let close_frame = CloseFrame {
                    code: axum::extract::ws::close_code::NORMAL,
                    reason: "Game already in progress".into(),
                };

                let _ = sender.send(Message::Close(Some(close_frame))).await;
                return;
            }
        }
        Err(e) => {
            tracing::error!("Failed to get lobby info for {}: {}", lobby_id, e);
            let close_frame = CloseFrame {
                code: axum::extract::ws::close_code::ABNORMAL,
                reason: "Failed to get lobby information".into(),
            };
            let _ = sender.send(Message::Close(Some(close_frame))).await;
            return;
        }
    }

    store_connection_and_send_queued_messages(player.id, lobby_id, sender, &connections, &redis)
        .await;

    if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
        let join_msg = LobbyServerMessage::PlayerUpdated { players };
        handler::broadcast_to_lobby(
            lobby_id,
            &join_msg,
            &connections,
            Some(&chat_connections),
            redis.clone(),
        )
        .await;
    }

    handler::handle_incoming_messages(
        receiver,
        lobby_id,
        &player,
        &connections,
        &chat_connections,
        redis.clone(),
    )
    .await;

    remove_connection(player.id, &connections).await;

    if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
        let msg = LobbyServerMessage::PlayerUpdated { players };
        handler::broadcast_to_lobby(lobby_id, &msg, &connections, Some(&chat_connections), redis)
            .await;
    }
}
