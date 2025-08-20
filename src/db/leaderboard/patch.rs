use crate::{errors::AppError, models::redis::RedisKey, state::RedisClient};
use redis::AsyncCommands;
use uuid::Uuid;

pub async fn _update_user_stats(
    user_id: Uuid,
    won: bool,
    pnl_change: f64,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let matches_key = RedisKey::users_matches();
    let wins_key = RedisKey::users_wins();
    let pnl_key = RedisKey::users_pnl();
    let user_id_str = user_id.to_string();

    // Use pipeline for efficiency
    let mut pipe = redis::pipe();

    // Increment match count
    pipe.cmd("ZINCRBY")
        .arg(&matches_key)
        .arg(1.0)
        .arg(&user_id_str);

    // Increment wins if won
    if won {
        pipe.cmd("ZINCRBY")
            .arg(&wins_key)
            .arg(1.0)
            .arg(&user_id_str);
    }

    // Update PnL if there's a change
    if pnl_change != 0.0 {
        pipe.cmd("HINCRBYFLOAT")
            .arg(&pnl_key)
            .arg(&user_id_str)
            .arg(pnl_change);
    }

    let _: () = pipe
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    tracing::info!(
        "Updated user stats for {}: won={}, pnl_change={}",
        user_id,
        won,
        pnl_change
    );

    Ok(())
}

/// Batch update stats for multiple users (useful for lobby completion)
pub async fn _batch_update_user_stats(
    user_stats: Vec<(Uuid, bool, f64)>, // (user_id, won, pnl_change)
    redis: RedisClient,
) -> Result<(), AppError> {
    if user_stats.is_empty() {
        return Ok(());
    }

    let user_count = user_stats.len();

    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let matches_key = RedisKey::users_matches();
    let wins_key = RedisKey::users_wins();
    let pnl_key = RedisKey::users_pnl();

    let mut pipe = redis::pipe();

    for (user_id, won, pnl_change) in user_stats {
        let user_id_str = user_id.to_string();

        // Increment match count for each user
        pipe.cmd("ZINCRBY")
            .arg(&matches_key)
            .arg(1.0)
            .arg(&user_id_str);

        // Increment wins if won
        if won {
            pipe.cmd("ZINCRBY")
                .arg(&wins_key)
                .arg(1.0)
                .arg(&user_id_str);
        }

        // Update PnL if there's a change
        if pnl_change != 0.0 {
            pipe.cmd("HINCRBYFLOAT")
                .arg(&pnl_key)
                .arg(&user_id_str)
                .arg(pnl_change);
        }
    }

    let _: () = pipe
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    tracing::info!("Batch updated stats for {} users", user_count);

    Ok(())
}

pub async fn _update_user_wars_point(
    user_id: Uuid,
    new_wars_point: f64,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let points_key = RedisKey::users_points();
    let user_id_str = user_id.to_string();

    // Update wars_point in sorted set
    let _: () = conn
        .zadd(&points_key, &user_id_str, new_wars_point)
        .await
        .map_err(AppError::RedisCommandError)?;

    tracing::info!(
        "Updated wars point for user {}: {}",
        user_id,
        new_wars_point
    );

    Ok(())
}
