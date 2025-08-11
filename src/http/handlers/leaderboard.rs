use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    db::{
        leaderboard::get::{get_leaderboard, get_user_stat},
        user::get::get_user_id,
    },
    models::leaderboard::LeaderBoard,
    state::AppState,
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

#[derive(Deserialize)]
pub struct GetUserStatPayload {
    pub user_id: Option<Uuid>,
    pub identifier: Option<String>,
}
pub async fn get_user_stat_handler(
    Query(payload): Query<GetUserStatPayload>,
    State(state): State<AppState>,
) -> Result<Json<LeaderBoard>, (StatusCode, String)> {
    let user_id = match (payload.user_id, payload.identifier) {
        (Some(id), _) => {
            // If user_id is provided, use it directly
            id
        }
        (None, Some(identifier)) => {
            // If identifier is provided, look up the user_id
            get_user_id(identifier, state.redis.clone())
                .await
                .map_err(|e| {
                    tracing::error!("Failed to get user ID from identifier: {}", e);
                    e.to_response()
                })?
        }
        (None, None) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Either user_id or identifier must be provided".to_string(),
            ));
        }
    };

    let user_stat = get_user_stat(user_id, state.redis).await.map_err(|e| {
        tracing::error!("Failed to get user stat for {}: {}", user_id, e);
        e.to_response()
    })?;

    Ok(Json(user_stat))
}
