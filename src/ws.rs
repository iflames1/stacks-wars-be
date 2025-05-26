use axum::{
    Router,
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
    routing::get,
};
use futures::{SinkExt, StreamExt};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;

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

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let rooms = state.rooms.clone();
    let connections = state.connections.clone();

    println!("New WebSocket connection from {}", addr);

    ws.on_upgrade(move |socket| handle_socket(socket, rooms, connections))
}

async fn handle_socket(stream: WebSocket, rooms: Rooms, connections: Connections) {
    let (sender, mut receiver) = stream.split();

    let player = Player {
        id: Uuid::new_v4(),
        username: None,
    };

    // Add player to a room
    {
        let mut locked_rooms = rooms.lock().await;

        if let Some((_id, room)) = locked_rooms.iter_mut().find(|(_, r)| r.players.len() == 1) {
            println!("Adding player {} to existing room {}", player.id, room.id);
            room.players.push(player.clone());
        } else {
            let room_id = Uuid::new_v4();
            let new_room = GameRoom {
                id: room_id,
                players: vec![player.clone()],
            };
            locked_rooms.insert(room_id, new_room);

            println!("Created new room {} for player {}", room_id, player.id);
        }
    }

    // Store sender (tx) for others to send to this player
    {
        let mut conns = connections.lock().await;
        conns.insert(player.id, Arc::new(Mutex::new(sender)));
    }

    // Clone handles for the receive task
    let connections_clone = connections.clone();
    let rooms_clone = rooms.clone();
    let player_id = player.id;

    tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(text) = msg {
                let room_id = {
                    let rooms_guard = rooms_clone.lock().await;
                    rooms_guard.iter().find_map(|(id, room)| {
                        if room.players.iter().any(|p| p.id == player_id) {
                            Some(*id)
                        } else {
                            None
                        }
                    })
                };

                if let Some(room_id) = room_id {
                    let rooms_guard = rooms_clone.lock().await;
                    let room = rooms_guard.get(&room_id).unwrap();

                    let connections_guard = connections_clone.lock().await;

                    for p in &room.players {
                        if p.id != player_id {
                            if let Some(sender_arc) = connections_guard.get(&p.id) {
                                let mut sender = sender_arc.lock().await;
                                let _ = sender
                                    .send(Message::Text(format!("{}: {}", player_id, text).into()))
                                    .await;
                            }
                        }
                    }
                }
            }
        }

        // On disconnect
        let mut conns = connections.lock().await;
        conns.remove(&player_id);
        println!("Player {} disconnected", player_id);
    });
}
