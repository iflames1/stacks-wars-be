use crate::{errors::AppError, state::RedisClient};
use redis::AsyncCommands;
use uuid::Uuid;

pub fn is_valid_username(username: &str) -> bool {
    let len_ok = (3..=20).contains(&username.len());
    let valid_chars = username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-');

    len_ok && valid_chars
}

pub async fn update_username(
    user_id: Uuid,
    new_username: String,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // Check if new username is taken by someone else
    let normalized = new_username.trim().to_lowercase();
    if !is_valid_username(&normalized) {
        return Err(AppError::BadRequest("Invalid username".into()));
    }
    let new_username_key = format!("username:{}", normalized);
    let existing_user_id: Option<String> = conn
        .get(&new_username_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    if let Some(existing) = existing_user_id {
        if existing != user_id.to_string() {
            return Err(AppError::BadRequest("Username already taken".into()));
        }
    }

    // Get the user's current username, if any
    let user_key = format!("user:{}", user_id);
    let current_username: Option<String> = conn
        .hget(&user_key, "username")
        .await
        .map_err(AppError::RedisCommandError)?;

    // Delete the old username key if it exists
    if let Some(old_username) = current_username {
        if old_username != new_username {
            let old_username_key = format!("username:{}", old_username);
            let _: () = conn
                .del(&old_username_key)
                .await
                .map_err(AppError::RedisCommandError)?;
        }
    }

    // Set the new username in user's hash
    let _: () = conn
        .hset(&user_key, "username", &new_username)
        .await
        .map_err(AppError::RedisCommandError)?;

    // Create the new username → user_id mapping
    let _: () = conn
        .set(&new_username_key, user_id.to_string())
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

pub async fn update_display_name(
    user_id: Uuid,
    new_display_name: String,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let trimmed = new_display_name.trim();
    if trimmed.is_empty() || trimmed.len() > 50 {
        return Err(AppError::BadRequest("Invalid display name".into()));
    }

    let user_key = format!("user:{}", user_id);

    let _: () = conn
        .hset(&user_key, "display_name", trimmed)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}
