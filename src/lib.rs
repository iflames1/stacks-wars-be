mod models;
mod state;
pub mod ws;

use state::{AppState, Connections, Rooms};
use std::net::SocketAddr;

pub async fn start_server() {
    tracing_subscriber::fmt::init();

    let rooms: Rooms = Default::default();
    let connections: Connections = Default::default();

    let state = AppState { rooms, connections };
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
