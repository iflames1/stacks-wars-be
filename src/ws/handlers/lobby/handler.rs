use axum::{
    extract::{ConnectInfo, Path, Query, State, WebSocketUpgrade, ws::WebSocket},
    response::IntoResponse,
};
use futures::StreamExt;
use serde::Deserialize;
use std::net::SocketAddr;

use crate::{
    db,
    models::{
        game::{Player, PlayerState},
        lobby::{JoinRequest, JoinState},
    },
    state::{AppState, LobbyJoinRequests, RedisClient},
    ws::handlers::lobby::message_handler,
};
use crate::{
    models::game::LobbyServerMessage,
    ws::handlers::utils::store_connection_and_send_queued_messages,
};
use crate::{state::ConnectionInfoMap, ws::handlers::utils::remove_connection};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct WsQueryParams {
    user_id: Uuid,
}

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
    let join_requests = state.lobby_join_requests.clone();

    let players = db::room::get_room_players(room_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    if let Some(matched_player) = players.iter().find(|p| p.id == player_id).cloned() {
        return Ok(ws.on_upgrade(move |socket| {
            handle_lobby_socket(
                socket,
                room_id,
                matched_player.into(),
                connections,
                redis,
                join_requests,
            )
        }));
    }

    let user = db::user::get_user_by_id(player_id, redis.clone())
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
        display_name: user.display_name.clone(),
        state: PlayerState::NotReady,
        rank: None,
        used_words: vec![],
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
            redis,
            join_requests,
        )
    }))
}

async fn handle_lobby_socket(
    socket: WebSocket,
    room_id: Uuid,
    player: Player,
    connections: ConnectionInfoMap,
    redis: RedisClient,
    join_requests: LobbyJoinRequests,
) {
    let (sender, receiver) = socket.split();

    store_connection_and_send_queued_messages(player.id, sender, &connections, &redis).await;

    if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
        let join_msg = LobbyServerMessage::PlayerJoined { players };
        message_handler::broadcast_to_lobby(room_id, &join_msg, &connections, redis.clone()).await;
    }

    message_handler::handle_incoming_messages(
        receiver,
        room_id,
        &player,
        &connections,
        redis.clone(),
        join_requests,
    )
    .await;

    remove_connection(player.id, &connections).await;

    if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
        let msg = LobbyServerMessage::PlayerLeft { players };
        message_handler::broadcast_to_lobby(room_id, &msg, &connections, redis).await;
    }
}
