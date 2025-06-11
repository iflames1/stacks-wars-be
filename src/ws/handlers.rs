use axum::{
    extract::{
        ConnectInfo, Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
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

use crate::games::lexi_wars::engine::handle_incoming_messages;
use crate::{
    db,
    games::lexi_wars::utils::{broadcast_to_room, generate_random_letter},
    models::game::{GameData, GameRoom, GameRoomInfo, GameState, Player, PlayerState},
    models::word_loader::WORD_LIST,
    state::{AppState, RedisClient},
};
use crate::{
    games::lexi_wars::rules::RuleContext,
    state::{Connections, Rooms},
};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct QueryParams {
    pub player_id: Uuid,
}

async fn store_connection(
    player: &Player,
    sender: SplitSink<WebSocket, Message>,
    connections: &Connections,
) {
    let mut conns = connections.lock().await;
    conns.insert(player.id, Arc::new(Mutex::new(sender)));
}

async fn remove_connection(player: &Player, connections: &Connections) {
    let mut conns = connections.lock().await;
    conns.remove(&player.id);
    println!("Player {} disconnected", player.wallet_address);
}

async fn setup_player_and_room(
    player: &Player,
    room_info: GameRoomInfo,
    players: Vec<Player>,
    rooms: &Rooms,
    connections: &Connections,
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
            rankings: vec![],
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
    rooms: Rooms,
    connections: Connections,
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
    Path(room_id): Path<Uuid>,
    Query(params): Query<QueryParams>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    println!("New WebSocket connection from {}", addr);

    let redis = state.redis.clone();
    let connections = state.connections.clone();

    // TODO: lets come back to this later
    let rooms = state.rooms.clone();

    let player_id = params.player_id;

    let room = match db::room::get_room_info(room_id, &redis).await {
        Some(room) => room,
        None => {
            println!("Room {} not found", room_id);
            return Err((axum::http::StatusCode::FORBIDDEN, "Room not found"));
        }
    };

    if room.state != GameState::InProgress {
        println!("Game in room {} is not in progress", room_id);
        return Err((axum::http::StatusCode::FORBIDDEN, "Game not in progress"));
    }

    let players = match db::room::get_room_players(room_id, &redis).await {
        Some(players) => players,
        None => {
            println!("No players found in room {}", room_id);
            return Err((
                axum::http::StatusCode::FORBIDDEN,
                "No players found in room",
            ));
        }
    };

    let players_clone = players.clone();
    let matched_player = match players
        .into_iter()
        .find(|p| p.id == player_id && p.state == PlayerState::Ready)
    {
        Some(player) => player,
        None => {
            println!(
                "Player with ID {} not found or not ready in room {}",
                player_id, room_id
            );
            return Err((
                axum::http::StatusCode::FORBIDDEN,
                "Player not found or not ready",
            ));
        }
    };

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
