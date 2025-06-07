use axum::{Router, routing::post};

use crate::{http::handlers::create_room_handler, state::AppState};

pub fn create_http_routes(state: AppState) -> Router {
    println!("Stacks Wars server running at http://127.0.0.1:3001/room");
    Router::new()
        .route("/room", post(create_room_handler))
        .with_state(state)
}
