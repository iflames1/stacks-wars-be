use axum::{
    Router,
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
    routing::get,
};
use futures::{SinkExt, StreamExt, stream::SplitSink};
use std::{collections::HashSet, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::sleep};

use crate::{models::GameRoom, state::AppState};
use crate::{
    models::Player,
    state::{Connections, Rooms},
};
use uuid::Uuid;

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
}

pub fn load_word_list() -> HashSet<String> {
    let json = include_str!("assets/words.json");
    serde_json::from_str(json).expect("Failed to parse words.json")
}

fn next_turn(players: &[Player], current_id: Uuid) -> Option<Uuid> {
    players.iter().position(|p| p.id == current_id).map(|i| {
        let next_index = (i + 1) % players.len(); // wrap around
        players[next_index].id
    })
}

pub async fn ws_handler(
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

async fn broadcast_to_room(
    sender_player: &Player,
    message: &str,
    room_id: Uuid,
    rooms: &Rooms,
    connections: &Connections,
) {
    let rooms_guard = rooms.lock().await;
    if let Some(room) = rooms_guard.get(&room_id) {
        let connection_guard = connections.lock().await;
        for p in &room.players {
            if let Some(sender_arc) = connection_guard.get(&p.id) {
                let mut sender = sender_arc.lock().await;
                let _ = sender
                    .send(Message::Text(
                        format!("{}: {}", sender_player.id, message).into(),
                    ))
                    .await;
            }
        }
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

fn start_turn_timer(
    player_id: Uuid,
    room_id: Uuid,
    rooms: Rooms,
    connections: Connections,
    words: Arc<HashSet<String>>,
) {
    tokio::spawn(async move {
        for i in (1..=10).rev() {
            {
                let rooms_guard = rooms.lock().await;
                if let Some(room) = rooms_guard.get(&room_id) {
                    if room.current_turn_id != player_id {
                        println!("turn changed, stopping timer");
                        return;
                    }
                } else {
                    println!("room not found, stopping timer");
                    return;
                }
            }

            let countdown_msg = format!("{} seconds left", i);
            broadcast_to_room(
                &Player {
                    id: player_id,
                    username: None,
                },
                &countdown_msg,
                room_id,
                &rooms,
                &connections,
            )
            .await;
            println!("{} seconds left for player {}", i, player_id);
            sleep(Duration::from_secs(1)).await;
        }

        // time ran out
        let mut rooms_guard = rooms.lock().await;
        if let Some(room) = rooms_guard.get_mut(&room_id) {
            if room.current_turn_id == player_id {
                println!("Player {} timed out", player_id);

                // Find index BEFORE removing the player
                let current_index = room.players.iter().position(|p| p.id == player_id);

                room.players.retain(|p| p.id != player_id);

                if room.players.is_empty() {
                    println!("Room {} is now empty", room.id);
                    return;
                }

                if let Some(idx) = current_index {
                    let next_index = if idx >= room.players.len() {
                        0 // wrap around if needed
                    } else {
                        idx
                    };
                    let next_id = room.players[next_index].id;
                    room.current_turn_id = next_id;

                    start_turn_timer(
                        next_id,
                        room_id,
                        rooms.clone(),
                        connections.clone(),
                        words.clone(),
                    );
                } else {
                    println!("Couldn't find timed-out player index in room {}", room.id);
                }
            }
        }
    });
}

async fn handle_incoming_messages(
    player: &Player,
    room_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    rooms: Rooms,
    connections: &Connections,
    words: Arc<HashSet<String>>,
) {
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            println!("Received from {}: {}", player.id, text);

            let cleaned_word = text.trim().to_lowercase();

            let advance_turn: bool;

            {
                let mut rooms_guard = rooms.lock().await;
                let room = rooms_guard.get_mut(&room_id).unwrap();

                // check turn
                if player.id != room.current_turn_id {
                    println!("Not {}'s turn", player.id);
                    continue;
                }

                // check if word is valid
                if !words.contains(&cleaned_word) {
                    println!("invalid word from {}: {}", player.id, cleaned_word);
                    continue;
                }

                // check if word is used
                if room.used_words.contains(&cleaned_word) {
                    println!("This word have been used: {}", cleaned_word);
                    continue;
                }

                // add to used words
                room.used_words.insert(cleaned_word.clone());

                // store next player id
                if let Some(next_id) = next_turn(&room.players, player.id) {
                    room.current_turn_id = next_id;
                }

                // start game loop
                start_turn_timer(
                    room.current_turn_id,
                    room_id,
                    rooms.clone(),
                    connections.clone(),
                    words.clone(),
                );

                advance_turn = true;
            }

            if advance_turn {
                broadcast_to_room(player, &cleaned_word, room_id, &rooms, connections).await;
            }
        }
    }

    //{
    //    let mut rooms_guard = rooms.lock().await;
    //    if let Some(room) = rooms_guard.get_mut(&room_id) {
    //        let was_current_turn = room.current_turn_id == player.id;

    //        println!("player {} timed out", player.id);

    //        // Remove the timed-out
    //        room.players.retain(|p| p.id != player.id);

    //        if room.players.is_empty() {
    //            println!("Room {} is empty", room.id);
    //            return;
    //        }

    //        if was_current_turn {
    //            if let Some(next_id) = next_turn(&room.players, player.id) {
    //                room.current_turn_id = next_id;
    //            } else {
    //                println!("No players left to take the turn in room {}", room.id);
    //            }
    //        }
    //    }
    //}
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
