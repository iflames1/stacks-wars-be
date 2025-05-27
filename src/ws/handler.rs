use axum::{
    Router,
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
    routing::get,
};
use futures::{StreamExt, stream::SplitSink};
use std::{collections::HashSet, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;

use crate::ws::game_loop::handle_incoming_messages;
use crate::{models::GameRoom, state::AppState};
use crate::{
    models::Player,
    state::{Connections, Rooms},
};
use uuid::Uuid;

fn load_word_list() -> HashSet<String> {
    let json = include_str!("../assets/words.json");
    serde_json::from_str(json).expect("Failed to parse words.json")
}

async fn setup_player_and_room(player: &Player, rooms: &Rooms) -> Uuid {
    let mut locked_rooms = rooms.lock().await;

    if let Some((id, room)) = locked_rooms.iter_mut().find(|(_, r)| r.players.len() == 1) {
        println!("Adding player {} to existing room {}", player.id, room.id);
        room.players.push(player.clone());
        *id
    } else {
        let room_id = Uuid::new_v4();
        let new_room = GameRoom {
            id: room_id,
            players: vec![player.clone()],
            current_turn_id: player.id,
            used_words: HashSet::new(),
        };
        locked_rooms.insert(room_id, new_room);

        println!("Created new room {} for player {}", room_id, player.id);
        room_id
    }
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

async fn handle_socket(
    stream: WebSocket,
    rooms: Rooms,
    connections: Connections,
    words: Arc<HashSet<String>>,
) {
    let (sender, receiver) = stream.split();

    let player = Player {
        id: Uuid::new_v4(),
        username: None,
    };

    // Add player to a room
    let room_id = setup_player_and_room(&player, &rooms).await;

    // Store sender (tx) for others to send to this player
    store_connection(&player, sender, &connections).await;

    handle_incoming_messages(&player, room_id, receiver, rooms, &connections, words).await;

    remove_connection(&player, &connections).await;
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let rooms = state.rooms.clone();
    let connections = state.connections.clone();

    println!("New WebSocket connection from {}", addr);

    let words = Arc::new(load_word_list());

    ws.on_upgrade(move |socket| handle_socket(socket, rooms, connections, words))
}

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
}
