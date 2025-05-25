use axum::{
    Router,
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
    routing::get,
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Debug, Clone)]
struct Player {
    id: Uuid,
    username: Option<String>,
}

struct GameRoom {
    id: Uuid,
    players: Vec<Player>,
}

type Rooms = Arc<Mutex<HashMap<Uuid, GameRoom>>>;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(rooms.clone());

    let addr = "127.0.0.1:3001";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    println!("Websocket server running at ws://{}", addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

async fn ws_handler(
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

        // Try to find a room with only 1 player
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
        // locked_rooms goes out of scope here, releasing the lock before any await
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
