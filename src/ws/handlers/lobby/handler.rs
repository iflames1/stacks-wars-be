use axum::{
    extract::{ConnectInfo, Path, Query, State, WebSocketUpgrade, ws::WebSocket},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;

use crate::{
    db::{
        lobby::get::{get_lobby_info, get_lobby_players},
        user::get::get_user_by_id,
    },
    models::{
        game::{LobbyState, Player, PlayerState, WsQueryParams},
        lobby::{JoinRequest, JoinState, LobbyServerMessage},
    },
    state::{AppState, ChatConnectionInfoMap, LobbyJoinRequests, RedisClient},
    ws::handlers::lobby::message_handler::handler,
};
use crate::{state::ConnectionInfoMap, ws::handlers::utils::remove_connection};
use crate::{
    state::LobbyCountdowns,
    ws::handlers::{
        lobby::message_handler::handler::send_error_to_player,
        utils::store_connection_and_send_queued_messages,
    },
};
use axum::extract::ws::{CloseFrame, Message};
use uuid::Uuid;

pub async fn lobby_ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    Path(room_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, String)> {
    tracing::info!("New lobby WS connection from {}", addr);

    let player_id = query.user_id;
    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let chat_connections = state.chat_connections.clone();
    let join_requests = state.lobby_join_requests.clone();
    let countdowns = state.lobby_countdowns.clone();

    let players = get_lobby_players(room_id, None, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    if let Some(matched_player) = players.iter().find(|p| p.id == player_id).cloned() {
        return Ok(ws.on_upgrade(move |socket| {
            handle_lobby_socket(
                socket,
                room_id,
                matched_player.into(),
                connections,
                chat_connections,
                redis,
                join_requests,
                countdowns,
            )
        }));
    }

    let user = get_user_by_id(player_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    {
        let mut guard = join_requests.lock().await;
        let room_requests = guard.entry(room_id).or_default();

        let already_requested = room_requests.iter().any(|req| req.user.id == user.id);
        if !already_requested {
            room_requests.push(JoinRequest {
                user: user.clone(),
                state: JoinState::Idle,
            });
        }
    }

    let idle_player = Player {
        id: user.id,
        wallet_address: user.wallet_address.clone(),
        username: user.username.clone(),
        display_name: user.display_name.clone(),
        wars_point: user.wars_point,
        state: PlayerState::NotReady,
        rank: None,
        used_words: None,
        tx_id: None,
        claim: None,
        prize: None,
    };

    Ok(ws.on_upgrade(move |socket| {
        handle_lobby_socket(
            socket,
            room_id,
            idle_player,
            connections,
            chat_connections,
            redis,
            join_requests,
            countdowns,
        )
    }))
}

async fn handle_lobby_socket(
    socket: WebSocket,
    room_id: Uuid,
    player: Player,
    connections: ConnectionInfoMap,
    chat_connections: ChatConnectionInfoMap,
    redis: RedisClient,
    join_requests: LobbyJoinRequests,
    countdowns: LobbyCountdowns,
) {
    let (mut sender, receiver) = socket.split();

    // Check room state immediately upon connection
    match get_lobby_info(room_id, redis.clone()).await {
        Ok(room_info) => {
            // Check if there's an active countdown
            let countdown_time = {
                let countdowns_guard = countdowns.lock().await;
                countdowns_guard
                    .get(&room_id)
                    .map(|state| state.current_time)
                    .unwrap_or(15)
            };

            // Always broadcast the current game state
            let ready_players =
                match get_lobby_players(room_id, Some(PlayerState::Ready), redis.clone()).await {
                    Ok(players) => players.into_iter().map(|p| p.id).collect::<Vec<_>>(),
                    Err(e) => {
                        tracing::error!("❌ Failed to get ready players: {}", e);
                        send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
                        vec![]
                    }
                };
            let game_state_msg = LobbyServerMessage::GameState {
                state: room_info.state.clone(),
                ready_players: Some(ready_players),
            };

            let serialized = match serde_json::to_string(&game_state_msg) {
                Ok(json) => json,
                Err(e) => {
                    tracing::error!("Failed to serialize GameState message: {}", e);
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

            // If game is in progress and countdown is 0, close the connection immediately
            if room_info.state == LobbyState::InProgress && countdown_time == 0 {
                tracing::info!(
                    "Player {} trying to connect to lobby while game is in progress (countdown finished) - closing connection",
                    player.wallet_address
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
            tracing::error!("Failed to get room info for {}: {}", room_id, e);
            let close_frame = CloseFrame {
                code: axum::extract::ws::close_code::ABNORMAL,
                reason: "Failed to get room information".into(),
            };
            let _ = sender.send(Message::Close(Some(close_frame))).await;
            return;
        }
    }

    store_connection_and_send_queued_messages(player.id, sender, &connections, &redis).await;

    if let Ok(players) = get_lobby_players(room_id, None, redis.clone()).await {
        let join_msg = LobbyServerMessage::PlayerUpdated { players };
        handler::broadcast_to_lobby(
            room_id,
            &join_msg,
            &connections,
            Some(&chat_connections),
            redis.clone(),
        )
        .await;
    }

    handler::handle_incoming_messages(
        receiver,
        room_id,
        &player,
        &connections,
        &chat_connections,
        redis.clone(),
        join_requests,
        countdowns,
    )
    .await;

    remove_connection(player.id, &connections).await;

    if let Ok(players) = get_lobby_players(room_id, None, redis.clone()).await {
        let msg = LobbyServerMessage::PlayerUpdated { players };
        handler::broadcast_to_lobby(room_id, &msg, &connections, Some(&chat_connections), redis)
            .await;
    }
}
