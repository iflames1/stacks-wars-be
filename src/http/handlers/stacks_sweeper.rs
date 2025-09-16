use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthClaims;
use crate::db::lobby::{get::get_stacks_sweeper_single, post::create_stacks_sweeper_single};
use crate::errors::AppError;
use crate::models::stacks_sweeper::MaskedCell;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateStacksSweeperPayload {
    pub size: usize,
    pub risk: f32,
    pub blind: bool,
    pub amount: f64,
    pub tx_id: String,
}

#[derive(Serialize)]
pub struct CreateStacksSweeperResponse {
    pub message: String,
    pub game_id: String,
}

#[derive(Serialize)]
pub struct GetStacksSweeperResponse {
    pub game_id: String,
    pub cells: Vec<MaskedCell>,
}

pub async fn create_stacks_sweeper_handler(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Json(payload): Json<CreateStacksSweeperPayload>,
) -> Result<Json<CreateStacksSweeperResponse>, (StatusCode, String)> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        tracing::error!("Unauthorized access attempt");
        AppError::Unauthorized("Invalid user ID in token".into()).to_response()
    })?;

    match create_stacks_sweeper_single(
        user_id,
        payload.size,
        payload.risk,
        payload.blind,
        payload.amount,
        payload.tx_id,
        state.redis,
    )
    .await
    {
        Ok(_) => {
            tracing::info!("StacksSweeper game created with ID: {}", user_id);
            Ok(Json(CreateStacksSweeperResponse {
                message: "StacksSweeper game created successfully".to_string(),
                game_id: user_id.to_string(),
            }))
        }
        Err(e) => {
            tracing::error!("Failed to create StacksSweeper game: {}", e);
            Err(e.to_response())
        }
    }
}

pub async fn get_stacks_sweeper_handler(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
) -> Result<Json<GetStacksSweeperResponse>, (StatusCode, String)> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
        tracing::error!("Unauthorized access attempt");
        AppError::Unauthorized("Invalid user ID in token".into()).to_response()
    })?;

    match get_stacks_sweeper_single(user_id, state.redis).await {
        Ok(masked_cells) => {
            tracing::info!("Retrieved StacksSweeper game for user: {}", user_id);
            Ok(Json(GetStacksSweeperResponse {
                game_id: user_id.to_string(),
                cells: masked_cells,
            }))
        }
        Err(e) => {
            tracing::error!("Failed to get StacksSweeper game: {}", e);
            Err(e.to_response())
        }
    }
}
