use axum::{
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use std::net::SocketAddr;

use crate::models::{Player, Rooms};
use uuid::Uuid;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(rooms): State<Rooms>,
) -> impl IntoResponse {
    println!("New WebSocket connection from {}", addr);

    ws.on_upgrade(move |socket| handle_socket(socket, rooms))
}

async fn handle_socket(mut socket: WebSocket, rooms: Rooms) {
    let player = Player {
        id: Uuid::new_v4(),
        username: None,
    };

    {
        let mut locked_rooms = rooms.lock().unwrap();

        if let Some((_id, room)) = locked_rooms.iter_mut().find(|(_, r)| r.players.len() == 1) {
            println!("Adding player {} to existing room {}", player.id, room.id);
            room.players.push(player.clone());
        } else {
            let room_id = Uuid::new_v4();
            let new_room = crate::models::GameRoom {
                id: room_id,
                players: vec![player.clone()],
            };
            locked_rooms.insert(room_id, new_room);

            println!("Created new room {} for player {}", room_id, player.id);
        }
    }

    while let Some(Ok(msg)) = socket.recv().await {
        if let Message::Text(text) = msg {
            println!("{} says: {}", player.id, text);
            let _ = socket
                .send(Message::Text(format!("Echo: {}", text).into()))
                .await;
        }
    }
}
