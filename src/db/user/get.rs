use crate::{errors::AppError, models::User, state::RedisClient};
use uuid::Uuid;

pub async fn get_user_by_id(id: Uuid, redis: RedisClient) -> Result<User, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("user:{}", id);

    let user_json: String = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    let user =
        serde_json::from_str(&user_json).map_err(|e| AppError::Deserialization(e.to_string()))?;

    Ok(user)
}
