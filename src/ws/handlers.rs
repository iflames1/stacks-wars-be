use axum::{
    extract::{
        ConnectInfo, Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures::{StreamExt, stream::SplitSink};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::Mutex;

use crate::{
    db,
    games::lexi_wars::utils::{broadcast_to_room, generate_random_letter},
    models::{
        game::{
            GameData, GameRoom, GameRoomInfo, GameState, LexiWarsServerMessage, Player, PlayerState,
        },
        lobby::{JoinRequest, JoinState},
        word_loader::WORD_LIST,
    },
    state::{AppState, LobbyJoinRequests, RedisClient},
    ws::lobby,
};
use crate::{errors::AppError, games::lexi_wars, models::game::LobbyServerMessage};
use crate::{
    games::lexi_wars::rules::RuleContext,
    state::{PlayerConnections, SharedRooms},
};
use uuid::Uuid;

// TODO: log to centralized logger
async fn store_connection(
    id: Uuid,
    sender: SplitSink<WebSocket, Message>,
    connections: &PlayerConnections,
) {
    let mut conns = connections.lock().await;
    conns.insert(id, Arc::new(Mutex::new(sender)));
}

pub async fn remove_connection(id: Uuid, connections: &PlayerConnections) {
    let mut conns = connections.lock().await;
    conns.remove(&id);
    //println!("Player {} disconnected", player.wallet_address);
}

async fn setup_player_and_room(
    player: &Player,
    room_info: GameRoomInfo,
    players: Vec<Player>,
    rooms: &SharedRooms,
    connections: &PlayerConnections,
) {
    let mut locked_rooms = rooms.lock().await;

    // Check if this room is already active in memory
    let room = locked_rooms.entry(room_info.id).or_insert_with(|| {
        println!("Initializing new in-memory GameRoom for {}", room_info.id);
        let word_list = WORD_LIST.clone();
        GameRoom {
            info: room_info.clone(),
            players: players.clone(),
            data: GameData::LexiWar { word_list },
            eliminated_players: vec![],
            current_turn_id: room_info.creator_id,
            used_words: HashMap::new(),
            used_words_global: HashSet::new(),
            rule_context: RuleContext {
                min_word_length: 4,
                random_letter: generate_random_letter(),
            },
            rule_index: 0,
        }
    });

    let already_exists = room.players.iter().any(|p| p.id == player.id);

    if !already_exists {
        println!(
            "Adding player {} ({}) to room {}",
            player.wallet_address, player.id, room.info.id
        );
        room.players.push(player.clone());
    } else {
        println!(
            "Player {} already exists in room {}, skipping re-add",
            player.wallet_address, room.info.id
        );
    }

    if let Some(current_player) = room.players.iter().find(|p| p.id == room.current_turn_id) {
        let next_turn_msg = LexiWarsServerMessage::Turn {
            current_turn: current_player.clone(),
        };
        broadcast_to_room(&next_turn_msg, &room, &connections).await;
    }
}

#[derive(Deserialize)]
pub struct WsQueryParams {
    user_id: Uuid,
}

pub async fn lexi_wars_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    Path(room_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::info!("New WebSocket connection from {}", addr);

    let player_id = query.user_id;

    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let rooms = state.rooms.clone();

    let room = db::room::get_room_info(room_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    if room.state != GameState::InProgress {
        return Err(AppError::BadRequest("Game not in progress".into()).to_response());
    }

    let players = db::room::get_room_players(room_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    // TODO: avoid cloning all players
    let players_clone = players.clone();

    let matched_player = players
        .into_iter()
        .find(|p| p.id == player_id && p.state == PlayerState::Ready)
        .ok_or_else(|| {
            AppError::Unauthorized("Player not found or not ready in room".into()).to_response()
        })?;

    tracing::info!(
        "Player {} allowed to join room {}",
        matched_player.wallet_address,
        room_id
    );

    Ok(ws.on_upgrade(move |socket| {
        let room_info = room.clone();
        handle_lexi_wars_socket(
            socket,
            room_id,
            matched_player,
            players_clone,
            rooms,
            connections,
            room_info,
            redis,
        )
    }))
}

async fn handle_lexi_wars_socket(
    socket: WebSocket,
    room_id: Uuid,
    player: Player,
    players: Vec<Player>,
    rooms: SharedRooms,
    connections: PlayerConnections,
    room_info: GameRoomInfo,
    redis: RedisClient,
) {
    let (sender, receiver) = socket.split();

    store_connection(player.id, sender, &connections).await;

    setup_player_and_room(&player, room_info, players, &rooms, &connections).await;

    lexi_wars::engine::handle_incoming_messages(
        &player,
        room_id,
        receiver,
        rooms,
        &connections,
        redis,
    )
    .await;

    remove_connection(player.id, &connections).await;
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
        tracing::info!(
            "Player {} joined lobby WS {}",
            matched_player.wallet_address,
            room_id
        );

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

    tracing::info!(
        "User {} is requesting to join lobby {}",
        user.wallet_address,
        room_id
    );

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
    connections: PlayerConnections,
    redis: RedisClient,
    join_requests: LobbyJoinRequests,
) {
    let (sender, receiver) = socket.split();

    store_connection(player.id, sender, &connections).await;

    if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
        let join_msg = LobbyServerMessage::PlayerJoined { players };
        lobby::broadcast_to_lobby(room_id, &join_msg, &connections, redis.clone()).await;
    }

    lobby::handle_incoming_messages(
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
        lobby::broadcast_to_lobby(room_id, &msg, &connections, redis).await;
    }
}
