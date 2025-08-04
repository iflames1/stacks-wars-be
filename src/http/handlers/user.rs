use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    db::user::{get::get_user_by_id, post::create_user},
    models::User,
    state::AppState,
};

#[derive(Deserialize)]
pub struct CreateUserPayload {
    pub wallet_address: String,
}

pub async fn create_user_handler(
    State(state): State<AppState>,
    Json(payload): Json<CreateUserPayload>,
) -> Result<Json<String>, (StatusCode, String)> {
    match create_user(payload.wallet_address.clone(), state.redis.clone()).await {
        Ok(token) => {
            tracing::info!(
                "User created with wallet address: {}",
                payload.wallet_address
            );
            Ok(Json(token))
        }
        Err(err) => {
            tracing::error!("Error creating user: {}", err);
            Err(err.to_response())
        }
    }
}

pub async fn get_user_handler(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<User>, (StatusCode, String)> {
    let user = get_user_by_id(user_id, state.redis.clone())
        .await
        .map_err(|e| {
            tracing::error!("Error retrieving user: {}", e);
            e.to_response()
        })?;

    tracing::info!("Retrieved user: {:?}", user);

    Ok(Json(user))
}
