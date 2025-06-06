mod models;
mod state;
pub mod ws;

use state::{AppState, Connections, Rooms};
use std::{net::SocketAddr, sync::Arc};

pub async fn start_server() {
    tracing_subscriber::fmt::init();

    let client = redis::Client::open("redis://127.0.0.1/").expect("Failed to create Redis client");
    let connection = client
        .get_connection_manager()
        .await
        .expect("Failed to connect to Redis");

    let redis = Arc::new(connection);

    let rooms: Rooms = Default::default();
    let connections: Connections = Default::default();

    let state = AppState {
        rooms,
        connections,
        redis,
    };
    let app = ws::create_app(state);

    let addr = "127.0.0.1:3001";
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind address");

    println!("Websocket server running at ws://{}", addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}
