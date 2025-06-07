use axum::{Router, routing::post};

use crate::{
    http::{
        create_room_handler, handlers::create_user_handler, join_room_handler, leave_room_handler,
    },
    state::AppState,
};

pub fn create_http_routes(state: AppState) -> Router {
    println!("Stacks Wars server running at http://127.0.0.1:3001/room");
    Router::new()
        .route("/user", post(create_user_handler))
        .route("/room", post(create_room_handler))
        .route("/room/join", post(join_room_handler))
        .route("/room/leave", post(leave_room_handler))
        .with_state(state)
}
