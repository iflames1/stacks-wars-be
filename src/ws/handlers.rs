use axum::{
    extract::{
        ConnectInfo, Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures::{StreamExt, stream::SplitSink};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::Mutex;

use crate::{
    auth::AuthClaims, errors::AppError, games::lexi_wars::engine::handle_incoming_messages,
};
use crate::{
    db,
    games::lexi_wars::utils::{broadcast_to_room, generate_random_letter},
    models::game::{GameData, GameRoom, GameRoomInfo, GameState, Player, PlayerState},
    models::word_loader::WORD_LIST,
    state::{AppState, RedisClient},
};
use crate::{
    games::lexi_wars::rules::RuleContext,
    state::{PlayerConnections, SharedRooms},
};
use uuid::Uuid;

// TODO: log to centralized logger
async fn store_connection(
    player: &Player,
    sender: SplitSink<WebSocket, Message>,
    connections: &PlayerConnections,
) {
    let mut conns = connections.lock().await;
    conns.insert(player.id, Arc::new(Mutex::new(sender)));
}

async fn remove_connection(player: &Player, connections: &PlayerConnections) {
    let mut conns = connections.lock().await;
    conns.remove(&player.id);
    println!("Player {} disconnected", player.wallet_address);
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
        broadcast_to_room(
            "current_turn",
            &current_player.wallet_address,
            &room,
            connections,
        )
        .await;
    }
}

async fn handle_socket(
    stream: WebSocket,
    room_id: Uuid,
    player: Player,
    players: Vec<Player>,
    rooms: SharedRooms,
    connections: PlayerConnections,
    room_info: GameRoomInfo,
    redis: RedisClient,
) {
    let (sender, receiver) = stream.split();

    store_connection(&player, sender, &connections).await;

    setup_player_and_room(&player, room_info, players, &rooms, &connections).await;

    handle_incoming_messages(&player, room_id, receiver, rooms, &connections, redis).await;

    remove_connection(&player, &connections).await;
}

#[axum::debug_handler]
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    AuthClaims(claims): AuthClaims,
    Path(room_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    println!("New WebSocket connection from {}", addr);

    let player_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| AppError::Unauthorized("Invalid user ID in token".into()).to_response())?;

    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let rooms = state.rooms.clone();

    let room = db::room::get_room_info(room_id, &redis)
        .await
        .ok_or_else(|| AppError::NotFound("Room not found".into()).to_response())?;

    if room.state != GameState::InProgress {
        return Err(AppError::BadRequest("Game not in progress".into()).to_response());
    }

    let players = db::room::get_room_players(room_id, &redis)
        .await
        .ok_or_else(|| AppError::NotFound("No players found in room".into()).to_response())?;

    // TODO: avoid cloning all players
    let players_clone = players.clone();

    let matched_player = players
        .into_iter()
        .find(|p| p.id == player_id && p.state == PlayerState::Ready)
        .ok_or_else(|| {
            AppError::Unauthorized("Player not found or not ready in room".into()).to_response()
        })?;

    println!(
        "Player {} allowed to join room {}",
        matched_player.wallet_address, room_id
    );

    Ok(ws.on_upgrade(move |socket| {
        let room_info = room.clone();
        handle_socket(
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
