use axum::{Router, routing::get};

use crate::{
    state::AppState,
    ws::handlers::{lexi_wars_handler, lobby_ws_handler},
};

pub fn create_ws_routes(state: AppState) -> Router {
    Router::new()
        .route("/ws/lexiwars/{room_id}", get(lexi_wars_handler))
        .route("/ws/room/{room_id}", get(lobby_ws_handler))
        .with_state(state)
}
