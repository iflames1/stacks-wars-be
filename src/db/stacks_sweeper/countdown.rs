use redis::AsyncCommands;
use uuid::Uuid;

use crate::{errors::AppError, state::RedisClient};

pub async fn set_stacks_sweeper_countdown(
    user_id: Uuid,
    time: u64,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("stacks_sweeper:{}:countdown", user_id);

    // Set countdown with 60 second expiration to auto-cleanup
    let _: () = conn
        .set_ex(&key, time, 60)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

pub async fn get_stacks_sweeper_countdown(
    user_id: Uuid,
    redis: RedisClient,
) -> Result<Option<u64>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("stacks_sweeper:{}:countdown", user_id);

    let time: Option<u64> = conn.get(&key).await.map_err(AppError::RedisCommandError)?;

    Ok(time)
}

pub async fn clear_stacks_sweeper_countdown(
    user_id: Uuid,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("stacks_sweeper:{}:countdown", user_id);

    let _: () = conn.del(&key).await.map_err(AppError::RedisCommandError)?;

    Ok(())
}

// Timer session management functions
pub async fn set_timer_session(
    user_id: Uuid,
    session_id: Uuid,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("stacks_sweeper:{}:timer_session", user_id);

    // Set timer session with 70 second expiration (slightly longer than countdown)
    let _: () = conn
        .set_ex(&key, session_id.to_string(), 70)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

pub async fn get_timer_session(
    user_id: Uuid,
    redis: RedisClient,
) -> Result<Option<Uuid>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("stacks_sweeper:{}:timer_session", user_id);

    let session_str: Option<String> = conn.get(&key).await.map_err(AppError::RedisCommandError)?;

    match session_str {
        Some(s) => Ok(Uuid::parse_str(&s).ok()),
        None => Ok(None),
    }
}

pub async fn clear_timer_session(user_id: Uuid, redis: RedisClient) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("stacks_sweeper:{}:timer_session", user_id);

    let _: () = conn.del(&key).await.map_err(AppError::RedisCommandError)?;

    Ok(())
}
