use sqlx::PgPool;

use crate::errors::AppError;

pub async fn get_current_season(postgres: PgPool) -> Result<i32, AppError> {
    let now = chrono::Utc::now();

    let current_season_id = sqlx::query_scalar::<_, i32>(
        "SELECT id
			FROM seasons
			WHERE start_date <= $1 AND end_date >= $1
			ORDER BY start_date DESC
			LIMIT 1",
    )
    .bind(now)
    .fetch_one(&postgres)
    .await
    .map_err(|e| AppError::DatabaseError(format!("Failed to fetch current season: {}", e)))?;

    Ok(current_season_id)
}
