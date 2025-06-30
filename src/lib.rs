pub mod auth;
mod db;
pub mod errors;
pub mod games;
mod http;
mod models;
mod state;
pub mod ws;

use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use state::{AppState, PlayerConnections, SharedRooms};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;

use crate::state::LobbyJoinRequests;

pub async fn start_server() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let manager = RedisConnectionManager::new("redis://127.0.0.1/").unwrap();
    let redis_pool = Pool::builder().build(manager).await.unwrap();

    let rooms: SharedRooms = Default::default();
    let connections: PlayerConnections = Default::default();
    let lobby_join_requests: LobbyJoinRequests = Arc::new(Mutex::new(HashMap::new()));

    let state = AppState {
        rooms,
        connections,
        redis: redis_pool,
        lobby_join_requests,
    };

    let ws_app = ws::create_ws_routes(state.clone());
    let http_app = http::create_http_routes(state);

    let app = ws_app.merge(http_app);

    let addr = "127.0.0.1:3001";
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind address");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}
