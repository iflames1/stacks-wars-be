use axum::{
    Router,
    routing::{post, put},
};

use crate::{
    http::{
        create_room_handler,
        handlers::{create_user_handler, update_game_state_handler, update_player_state_handler},
        join_room_handler, leave_room_handler,
    },
    state::AppState,
};

pub fn create_http_routes(state: AppState) -> Router {
    println!("Stacks Wars server running at http://127.0.0.1:3001/room");
    Router::new()
        .route("/user", post(create_user_handler))
        .route("/room", post(create_room_handler))
        .route("/room/{room_id}/join", put(join_room_handler))
        .route("/room/{room_id}/leave", put(leave_room_handler))
        .route("/room/{room_id}/state", put(update_game_state_handler))
        .route(
            "/room/{room_id}/player-state",
            put(update_player_state_handler),
        )
        .with_state(state)
}
