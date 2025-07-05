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
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

use crate::state::LobbyJoinRequests;

pub async fn start_server() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let redis_url = std::env::var("REDIS_URL").expect("REDIS_URL must be set");
    let manager = RedisConnectionManager::new(redis_url).unwrap();

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

    let port = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(3001);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("Failed to bind address");

    axum::serve(listener, app).await.unwrap();
}
