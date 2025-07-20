use crate::{auth::generate_jwt, errors::AppError, models::User, state::RedisClient};
use uuid::Uuid;

pub async fn create_user(
    wallet_address: String,
    display_name: Option<String>,
    redis: RedisClient,
) -> Result<String, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let wallet_key = format!("wallet:{}", wallet_address);
    let existing_id: Option<String> = redis::cmd("GET")
        .arg(&wallet_key)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    if let Some(existing_id) = existing_id {
        let user_key = format!("user:{}", existing_id);
        let user_json: Option<String> = redis::cmd("GET")
            .arg(&user_key)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;

        if let Some(user_json) = user_json {
            let user: User = serde_json::from_str(&user_json)
                .map_err(|e| AppError::Serialization(e.to_string()))?;
            let token = generate_jwt(&user)?;
            return Ok(token);
        } else {
            return Err(AppError::BadRequest(
                "Wallet mapped but user not found".into(),
            ));
        }
    }

    let user = User {
        id: Uuid::new_v4(),
        wallet_address: wallet_address.clone(),
        display_name,
    };

    let user_key = format!("user:{}", user.id);
    let json =
        serde_json::to_string(&user).map_err(|e| AppError::Deserialization(e.to_string()))?;

    let _: () = redis::cmd("SET")
        .arg(&user_key)
        .arg(json)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    let _: () = redis::cmd("SET")
        .arg(&wallet_key)
        .arg(user.id.to_string())
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    let token = generate_jwt(&user)?;
    Ok(token)
}
