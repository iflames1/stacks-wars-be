mod db;
mod http;
mod models;
mod state;
pub mod ws;

use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use state::{AppState, Connections, Rooms};
use std::net::SocketAddr;

pub async fn start_server() {
    tracing_subscriber::fmt::init();

    let manager = RedisConnectionManager::new("redis://127.0.0.1/").unwrap();

    let redis_pool = Pool::builder().build(manager).await.unwrap();

    let rooms: Rooms = Default::default();
    let connections: Connections = Default::default();

    let state = AppState {
        rooms,
        connections,
        redis: redis_pool,
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
