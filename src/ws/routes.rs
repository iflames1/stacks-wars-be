use axum::{Router, routing::get};

use crate::{
    state::AppState,
    ws::handlers::{lexi_wars_handler, lobby_ws_handler},
};

pub fn create_ws_routes(state: AppState) -> Router {
    println!("Stacks Wars websocket running at ws://127.0.0.1:3001/ws/room_id");
    Router::new()
        .route("/lexi-wars/{room_id}", get(lexi_wars_handler))
        .route("/room/{room_id}", get(lobby_ws_handler))
        .with_state(state)
}
