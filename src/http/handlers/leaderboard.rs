use axum::{extract::State, http::StatusCode, response::Json};

use crate::{
    db::leaderboard::get::get_leaderboard, models::leaderboard::LeaderBoard, state::AppState,
};

pub async fn get_leaderboard_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<LeaderBoard>>, (StatusCode, String)> {
    let leaderboard = get_leaderboard(state.redis).await.map_err(|e| {
        tracing::error!("Failed to get leaderboard: {}", e);
        e.to_response()
    })?;

    Ok(Json(leaderboard))
}
