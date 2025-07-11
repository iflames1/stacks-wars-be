use axum::{
    Router,
    routing::{get, post, put},
};

use crate::{
    http::{
        create_room_handler,
        handlers::{
            add_game_handler, create_user_handler, get_all_games_handler, get_all_rooms_handler,
            get_game_handler, get_players_handler, get_room_extended_handler, get_room_handler,
            get_rooms_by_game_id_handler, get_user_handler, kick_player_handler,
            update_claim_state_handler, update_game_state_handler, update_player_state_handler,
        },
        join_room_handler, leave_room_handler,
    },
    state::AppState,
};

pub fn create_http_routes(state: AppState) -> Router {
    Router::new()
        .route("/user", post(create_user_handler))
        .route("/user/{user_id}", get(get_user_handler))
        .route("/room", post(create_room_handler))
        .route("/room/{room_id}/extended", get(get_room_extended_handler))
        .route("/rooms/{game_id}", get(get_rooms_by_game_id_handler))
        .route("/room/{room_id}/join", put(join_room_handler))
        .route("/room/{room_id}/leave", put(leave_room_handler))
        .route("/room/{room_id}/kick", put(kick_player_handler))
        .route("/room/{room_id}/state", put(update_game_state_handler))
        .route(
            "/room/{room_id}/player-state",
            put(update_player_state_handler),
        )
        .route(
            "/room/{room_id}/claim-state",
            put(update_claim_state_handler),
        )
        .route("/game", post(add_game_handler))
        .route("/games", get(get_all_games_handler))
        .route("/game/{game_id}", get(get_game_handler))
        .route("/room/{room_id}", get(get_room_handler))
        .route("/rooms", get(get_all_rooms_handler))
        .route("/room/{room_id}/players", get(get_players_handler))
        .with_state(state)
}
