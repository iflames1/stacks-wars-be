use redis::AsyncCommands;
use uuid::Uuid;

use crate::{
    errors::AppError,
    models::{
        game::Player,
        redis::{KeyPart, RedisKey},
    },
    state::RedisClient,
};

pub async fn update_connected_players(
    lobby_id: Uuid,
    connected_players: Vec<Player>,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    for player in connected_players {
        let player_key =
            RedisKey::lobby_connected_player(KeyPart::Id(lobby_id), KeyPart::Id(player.id));
        let hash = player.to_redis_hash();
        let fields: Vec<(&str, &str)> =
            hash.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let _: () = conn
            .hset_multiple(&player_key, &fields)
            .await
            .map_err(AppError::RedisCommandError)?;
    }

    Ok(())
}
