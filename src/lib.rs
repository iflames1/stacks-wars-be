pub mod auth;
mod db;
pub mod errors;
pub mod games;
mod http;
mod models;
mod state;
pub mod ws;

use axum::Router;
use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use state::{AppState, PlayerConnections, SharedRooms};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use teloxide::Bot;
use tokio::sync::Mutex;

use crate::state::LobbyJoinRequests;

pub async fn start_server() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let redis_url = std::env::var("REDIS_URL").expect("REDIS_URL must be set");
    let manager = RedisConnectionManager::new(redis_url).unwrap();

    let bot_token = std::env::var("TELEGRAM_BOT_TOKEN").expect("TELEGRAM_BOT_TOKEN must be set");
    let bot = Bot::new(bot_token);

    let redis_pool = Pool::builder().build(manager).await.unwrap();
    let rooms: SharedRooms = Default::default();
    let connections: PlayerConnections = Default::default();
    let lobby_join_requests: LobbyJoinRequests = Arc::new(Mutex::new(HashMap::new()));

    let state = AppState {
        rooms,
        connections,
        redis: redis_pool,
        lobby_join_requests,
        bot,
    };

    let app = Router::new()
        .merge(http::create_http_routes(state.clone()))
        .merge(ws::create_ws_routes(state))
        .fallback(|| async { "404 Not Found" });

    let port = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(3001);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("Failed to bind address");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

//async fn cleanup_stale_connections(connections: &PlayerConnections) {
//    let mut conns = connections.lock().await;
//    let mut to_remove = Vec::new();

//    for (player_id, conn_info) in conns.iter() {
//        let last_seen = conn_info.last_seen.lock().await;
//        let is_healthy = conn_info.is_healthy.lock().await;

//        // Remove connections that haven't been seen for 5 minutes or are unhealthy
//        if last_seen.elapsed().as_secs() > 300 || !*is_healthy {
//            to_remove.push(*player_id);
//        }
//    }

//    for player_id in to_remove {
//        tracing::info!("Cleaning up stale connection for player {}", player_id);
//        conns.remove(&player_id);
//    }
//}
