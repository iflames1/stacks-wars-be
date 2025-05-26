mod models;
mod state;
mod ws;

use std::net::SocketAddr;

use state::{AppState, Connections, Rooms};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let rooms: Rooms = Default::default();
    let connections: Connections = Default::default();

    let state = AppState { rooms, connections };

    let app = ws::create_app(state);

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
