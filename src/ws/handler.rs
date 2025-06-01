use axum::{
    Router,
    extract::{
        ConnectInfo, Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
    routing::get,
};
use futures::{StreamExt, stream::SplitSink};
use rand::{Rng, rng};
use std::{collections::HashSet, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;

use crate::{models::GameRoom, state::AppState};
use crate::{
    models::Player,
    state::{Connections, Rooms},
};
use crate::{
    models::QueryParams,
    ws::{game_loop::handle_incoming_messages, rules::RuleContext},
};
use uuid::Uuid;

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
    println!("Player {} disconnected", player.id);
}

async fn setup_player_and_room(player: &Player, rooms: &Rooms, room_id: Uuid) {
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

    room.players.push(player.clone());
    println!(
        "Player {} ({}) joined room {} ({} players)",
        player.username.as_deref().unwrap_or("Unknown"),
        player.id,
        room_id,
        room.players.len()
    );
}

async fn handle_socket(
    stream: WebSocket,
    room_id: Uuid,
    username: Option<String>,
    rooms: Rooms,
    connections: Connections,
    words: Arc<HashSet<String>>,
) {
    let (sender, receiver) = stream.split();

    let player = Player {
        id: Uuid::new_v4(),
        username,
    };

    // Add to the specified room (create if missing)
    setup_player_and_room(&player, &rooms, room_id).await;

    // Store sender (tx) for others to send to this player
    store_connection(&player, sender, &connections).await;

    handle_incoming_messages(&player, room_id, receiver, rooms, &connections, words).await;

    remove_connection(&player, &connections).await;
}

#[axum::debug_handler]
async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(room_id): Path<Uuid>,
    Query(params): Query<QueryParams>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let rooms = state.rooms;
    let connections = state.connections;

    println!("New WebSocket connection from {}", addr);

    let username = params.username;
    let words = Arc::new(load_word_list());

    ws.on_upgrade(move |socket| handle_socket(socket, room_id, username, rooms, connections, words))
}

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/ws/{room_id}", get(ws_handler))
        .with_state(state)
}
