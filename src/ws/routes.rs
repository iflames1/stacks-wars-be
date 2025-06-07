use axum::{Router, routing::get};

use crate::{state::AppState, ws::handlers::ws_handler};

pub fn create_ws_routes(state: AppState) -> Router {
    println!("Stacks Wars websocket running at ws://127.0.0.1:3001/ws/room_id");
    Router::new()
        .route("/ws/{room_id}", get(ws_handler))
        .with_state(state)
}
