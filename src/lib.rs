pub mod auth;
mod db;
pub mod errors;
pub mod games;
mod http;
mod middleware;
mod models;
mod state;
pub mod ws;

use axum::{Router, middleware as axum_middleware};
use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use middleware::{cors_layer, create_global_rate_limiter, rate_limit_middleware};
use state::{AppState, ChatConnectionInfoMap, ConnectionInfoMap};
use std::net::SocketAddr;
use teloxide::Bot;

use crate::games::init::initialize_games;

pub async fn start_server() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let redis_url = std::env::var("REDIS_URL").expect("REDIS_URL must be set");
    let manager = RedisConnectionManager::new(redis_url).unwrap();

    let bot_token = std::env::var("TELEGRAM_BOT_TOKEN").expect("TELEGRAM_BOT_TOKEN must be set");
    let bot = Bot::new(bot_token);

    let redis_pool = Pool::builder().build(manager).await.unwrap();

    // Initialize games in database
    if let Err(e) = initialize_games(redis_pool.clone()).await {
        tracing::error!("Failed to initialize games: {}", e);
        panic!("Failed to initialize games: {}", e);
    }

    let connections: ConnectionInfoMap = Default::default();
    let chat_connections: ChatConnectionInfoMap = Default::default();
    let state = AppState {
        connections,
        chat_connections,
        redis: redis_pool,
        bot,
    };

    // Create rate limiters
    let global_rate_limiter = create_global_rate_limiter();

    let app = Router::new()
        .merge(http::create_http_routes(state.clone()))
        .merge(ws::create_ws_routes(state))
        .layer(axum_middleware::from_fn(move |req, next| {
            rate_limit_middleware(global_rate_limiter.clone(), req, next)
        }))
        .layer(cors_layer())
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
