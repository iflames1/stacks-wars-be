use axum::{
    extract::{
        ConnectInfo, Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use futures::{StreamExt, stream::SplitSink};
use rand::{Rng, rng};
//use redis::AsyncCommands;
use std::{collections::HashSet, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;

use crate::{
    models::GameRoom,
    state::{AppState, RedisClient},
};
use crate::{
    models::Player,
    state::{Connections, Rooms},
};
use crate::{
    models::QueryParams,
    ws::{game_loop::handle_incoming_messages, rules::RuleContext},
};
use uuid::Uuid;

use super::game_loop::broadcast_to_room;

fn load_word_list() -> HashSet<String> {
    let json = include_str!("../assets/words.json");
    serde_json::from_str(json).expect("Failed to parse words.json")
}

pub fn generate_random_letter() -> char {
    let letter = rng().random_range(0..26);
    (b'a' + letter as u8) as char
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
    println!("Player {} disconnected", player.username);
}

async fn setup_player_and_room(
    player: &Player,
    rooms: &Rooms,
    room_id: Uuid,
    connections: &Connections,
) {
    let mut locked_rooms = rooms.lock().await;

    let room = locked_rooms.entry(room_id).or_insert_with(|| GameRoom {
        id: room_id,
        players: vec![],
        eliminated_players: vec![],
        current_turn_id: player.id,
        used_words: HashSet::new(),
        rule_context: RuleContext {
            min_word_length: 4,
            random_letter: generate_random_letter(),
        },
        rule_index: 0,
        game_over: false,
        rankings: vec![],
    });

    let already_exists = room.players.iter().any(|p| p.username == player.username);

    if !already_exists {
        room.players.push(player.clone());
        println!(
            "Player {} ({}) joined room {} ({} players)",
            player.username,
            player.id,
            room_id,
            room.players.len()
        );
    } else {
        println!(
            "Player {} already in room {}, skipping re-add",
            player.username, room_id
        );
    }

    // unexpected behavior: does send current turn to only previous joiners
    if let Some(current_player) = room.players.iter().find(|p| p.id == room.current_turn_id) {
        broadcast_to_room(
            "current_turn",
            &current_player.username,
            &room,
            &connections,
        )
        .await;
    }
}

async fn handle_socket(
    stream: WebSocket,
    room_id: Uuid,
    username: String,
    rooms: Rooms,
    connections: Connections,
    _redis: RedisClient,
    words: Arc<HashSet<String>>,
) {
    let (sender, receiver) = stream.split();

    let player = Player {
        id: Uuid::new_v4(),
        username,
    };

    // Save player to Redis
    //let player_key = format!("player:{}", player.id);
    //let mut conn = redis.get().await.unwrap();
    //let _: () = conn.set(&player_key, &player.username).await.unwrap();

    // Store sender (tx) for others to send to this player
    store_connection(&player, sender, &connections).await;

    // Add to the specified room (create if missing)
    setup_player_and_room(&player, &rooms, room_id, &connections).await;

    handle_incoming_messages(&player, room_id, receiver, rooms, &connections, words).await;

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
    let rooms = state.rooms;
    let connections = state.connections;

    println!("New WebSocket connection from {}", addr);

    let redis = state.redis.clone();
    let username = params.username;
    let words = Arc::new(load_word_list());

    ws.on_upgrade(move |socket| {
        handle_socket(socket, room_id, username, rooms, connections, redis, words)
    })
}
