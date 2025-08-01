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
use state::AppState;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use teloxide::Bot;
use tokio::sync::Mutex;

use crate::games::init::initialize_games;

pub async fn start_server() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let redis_url = std::env::var("REDIS_URL").expect("REDIS_URL must be set");
    let manager = RedisConnectionManager::new(redis_url).expect("Failed to create Redis manager");
    let redis = Pool::builder()
        .build(manager)
        .await
        .expect("Failed to create Redis pool");

    let telegram_bot = std::env::var("TELEGRAM_BOT_TOKEN")
        .ok()
        .map(Bot::new)
        .expect("TELEGRAM_BOT_TOKEN must be set");

    // Initialize mediasoup worker
    let worker_manager = Arc::new(mediasoup::worker_manager::WorkerManager::new());

    tracing::info!("Mediasoup worker created successfully");

    let state = AppState {
        redis,
        connections: Arc::new(Mutex::new(HashMap::new())),
        chat_connections: Arc::new(Mutex::new(HashMap::new())),
        chat_histories: Arc::new(Mutex::new(HashMap::new())),
        rooms: Arc::new(Mutex::new(HashMap::new())),
        lobby_join_requests: Arc::new(Mutex::new(HashMap::new())),
        lobby_countdowns: Arc::new(Mutex::new(HashMap::new())),
        telegram_bot,
        voice_connections: Arc::new(Mutex::new(HashMap::new())),
        worker_manager,
    };

    // Initialize games in database
    if let Err(e) = initialize_games(state.redis.clone()).await {
        tracing::error!("Failed to initialize games: {}", e);
        panic!("Failed to initialize games: {}", e);
    }

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
