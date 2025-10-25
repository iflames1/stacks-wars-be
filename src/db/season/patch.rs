use crate::{errors::AppError, models::season::Season};
use chrono::NaiveDateTime;
use sqlx::PgPool;

pub async fn add_season(
    name: String,
    description: Option<String>,
    start_date: NaiveDateTime,
    end_date: NaiveDateTime,
    postgres: PgPool,
) -> Result<Season, AppError> {
    // Validate dates
    if end_date <= start_date {
        return Err(AppError::BadRequest(
            "End date must be after start date".into(),
        ));
    }

    // Check if a season with the same name already exists
    let existing_season = sqlx::query_as::<_, (i32,)>("SELECT id FROM seasons WHERE name = $1")
        .bind(&name)
        .fetch_optional(&postgres)
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to query season: {}", e)))?;

    if existing_season.is_some() {
        return Err(AppError::BadRequest(format!(
            "Season with name '{}' already exists",
            name
        )));
    }

    // Insert new season
    let season = sqlx::query_as::<_, Season>(
        "INSERT INTO seasons (name, description, start_date, end_date)
        VALUES ($1, $2, $3, $4)
        RETURNING id, name, description, start_date, end_date, created_at",
    )
    .bind(&name)
    .bind(&description)
    .bind(start_date)
    .bind(end_date)
    .fetch_one(&postgres)
    .await
    .map_err(|e| AppError::DatabaseError(format!("Failed to create season: {}", e)))?;

    tracing::info!("Created new season: {} (ID: {})", season.name, season.id);

    Ok(season)
}
