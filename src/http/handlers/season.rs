use axum::{Json, extract::State, http::StatusCode};
use chrono::NaiveDateTime;
use serde::Deserialize;

use crate::{db::season::patch::add_season, models::season::Season, state::AppState};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSeasonPayload {
    pub name: String,
    pub description: Option<String>,
    pub start_date: String,
    pub end_date: String,
}

pub async fn add_season_handler(
    State(state): State<AppState>,
    Json(payload): Json<CreateSeasonPayload>,
) -> Result<Json<Season>, (StatusCode, String)> {
    let start_date = NaiveDateTime::parse_from_str(&payload.start_date, "%Y-%m-%d %H:%M:%S")
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!(
                    "Invalid start_date format: {}. Expected format: YYYY-MM-DD HH:MM:SS",
                    e
                ),
            )
        })?;

    let end_date =
        NaiveDateTime::parse_from_str(&payload.end_date, "%Y-%m-%d %H:%M:%S").map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!(
                    "Invalid end_date format: {}. Expected format: YYYY-MM-DD HH:MM:SS",
                    e
                ),
            )
        })?;

    match add_season(
        payload.name.clone(),
        payload.description,
        start_date,
        end_date,
        state.postgres.clone(),
    )
    .await
    {
        Ok(season) => {
            tracing::info!("Season created: {} (ID: {})", season.name, season.id);
            Ok(Json(season))
        }
        Err(err) => {
            tracing::error!("Error creating season: {}", err);
            Err(err.to_response())
        }
    }
}
