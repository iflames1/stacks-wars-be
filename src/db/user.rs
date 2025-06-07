use crate::{models::User, state::RedisClient};
use uuid::Uuid;

pub async fn create_user(
    wallet_address: String,
    display_name: Option<String>,
    redis: RedisClient,
) -> Result<User, String> {
    let user = User {
        id: Uuid::new_v4(),
        wallet_address,
        display_name,
    };

    let mut conn = redis
        .get()
        .await
        .map_err(|_| "Failed to get Redis connection")?;
    let user_key = format!("user:{}", user.id);

    let json = serde_json::to_string(&user).map_err(|_| "Serialization error")?;

    let _: () = redis::cmd("SET")
        .arg(user_key)
        .arg(json)
        .query_async(&mut *conn)
        .await
        .map_err(|_| "Failed to store user")?;

    Ok(user)
}

pub async fn get_user_by_id(id: Uuid, redis: RedisClient) -> Option<User> {
    let mut conn = redis.get().await.ok()?;
    let key = format!("user:{}", id);

    let user_json: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .ok()?;

    user_json.and_then(|json| serde_json::from_str(&json).ok())
}
