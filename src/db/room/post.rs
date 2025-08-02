use chrono::Utc;
use redis::AsyncCommands;
use teloxide::Bot;
use uuid::Uuid;

use crate::{
    db::{tx::validate_payment_tx, user::get_user_by_id},
    errors::AppError,
    http::bot::{self, BotNewLobbyPayload},
    models::game::{
        GameType, LobbyInfo, LobbyPool, LobbyPoolInput, LobbyState, Player, PlayerState,
    },
    state::RedisClient,
};

pub async fn create_lobby(
    name: String,
    description: Option<String>,
    creator_id: Uuid,
    game_id: Uuid,
    game_name: String,
    pool: Option<LobbyPoolInput>,
    redis: RedisClient,
    bot: Bot,
) -> Result<Uuid, AppError> {
    let lobby_id = Uuid::new_v4();
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let lobby_info = LobbyInfo {
        id: lobby_id,
        name,
        description,
        creator_id,
        state: LobbyState::Waiting,
        game_id,
        game_name,
        participants: 1,
        contract_address: pool.as_ref().map(|p| p.contract_address.clone()),
        created_at: Utc::now(),
    };

    let creator_user = get_user_by_id(creator_id, redis.clone()).await?;

    // Store pool if it exists
    if let Some(pool_input) = &pool {
        validate_payment_tx(
            &pool_input.tx_id,
            &creator_user.wallet_address,
            &pool_input.contract_address,
            pool_input.entry_amount,
        )
        .await?;

        let pool_struct = LobbyPool {
            entry_amount: pool_input.entry_amount,
            contract_address: pool_input.contract_address.clone(),
            current_amount: pool_input.entry_amount,
        };

        let pool_key = format!("lobby:{}:pool", lobby_id);

        let pool_hash = pool_struct.to_redis_hash();
        let fields: Vec<(&str, &str)> = pool_hash
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let _: () = conn
            .hset_multiple(&pool_key, &fields)
            .await
            .map_err(AppError::RedisCommandError)?;
    }

    let lobby_key = format!("lobby:{}", lobby_id);
    let player_key = format!("lobby:{}:player:{}", lobby_id, creator_user.id);
    let lobby_player = Player {
        id: creator_user.id,
        wallet_address: creator_user.wallet_address.clone(),
        display_name: creator_user.display_name.clone(),
        username: creator_user.username.clone(),
        state: PlayerState::Ready,
        used_words: None,
        rank: None,
        tx_id: pool.as_ref().map(|p| p.tx_id.to_owned()),
        claim: None,
        prize: None,
        wars_point: creator_user.wars_point,
    };
    let player_hash = lobby_player.to_redis_hash();

    let lobby_fields = lobby_info.to_redis_hash();

    let created_score = lobby_info.created_at.timestamp();

    let _: () = redis::pipe()
        .cmd("HSET")
        .arg(&lobby_key)
        .arg(
            lobby_fields
                .iter()
                .flat_map(|(k, v)| [k.as_ref(), v.as_str()])
                .collect::<Vec<&str>>(),
        )
        .ignore()
        .cmd("HSET")
        .arg(&player_key)
        .arg(
            player_hash
                .iter()
                .flat_map(|(k, v)| [k.as_ref(), v.as_str()])
                .collect::<Vec<&str>>(),
        )
        .ignore()
        .cmd("ZADD")
        .arg("lobbies:all")
        .arg(created_score)
        .arg(lobby_id.to_string())
        .ignore()
        .cmd("ZADD")
        .arg(format!("lobbies:{:?}", LobbyState::Waiting).to_lowercase())
        .arg(created_score)
        .arg(lobby_id.to_string())
        .ignore()
        .cmd("ZADD")
        .arg(format!("game:{}:lobbies", game_id))
        .arg(created_score)
        .arg(lobby_id.to_string())
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    let redis_clone = redis.clone();
    tokio::spawn(async move {
        let game_key = format!("game:{}:data", game_id);
        let game_data_json: Option<String> = match redis_clone.get().await {
            Ok(mut conn) => match conn.get(game_key).await {
                Ok(json) => Some(json),
                Err(e) => {
                    tracing::warn!("Could not fetch game data for Telegram bot: {}", e);
                    None
                }
            },
            Err(e) => {
                tracing::warn!("Could not connect to Redis: {}", e);
                None
            }
        };

        let game_image = game_data_json
            .as_ref()
            .and_then(|json| serde_json::from_str::<GameType>(json).ok())
            .map(|g| g.image_url)
            .unwrap_or_default();

        let payload = BotNewLobbyPayload {
            lobby_id,
            lobby_name: lobby_info.name.clone(),
            description: lobby_info.description.clone(),
            game_name: lobby_info.game_name.clone(),
            game_image,
            entry_amount: pool.as_ref().and_then(|p| Some(p.entry_amount)),
            contract_address: lobby_info.contract_address.clone(),
            creator_name: creator_user.display_name.or(creator_user.username),
            wallet_address: creator_user.wallet_address.clone(),
        };

        let chat_id = std::env::var("TELEGRAM_CHAT_ID")
            .expect("TELEGRAM_CHAT_ID must be set")
            .parse::<i64>()
            .unwrap();

        if let Err(e) = bot::broadcast_lobby_created(&bot, chat_id, payload).await {
            tracing::error!("Failed to broadcast lobby creation: {}", e);
        }
    });

    Ok(lobby_id)
}
