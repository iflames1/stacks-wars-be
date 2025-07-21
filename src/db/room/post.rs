use redis::AsyncCommands;
use teloxide::Bot;
use uuid::Uuid;

use crate::{
    db::{tx::validate_payment_tx, user::get_user_by_id},
    errors::AppError,
    http::bot::{self, BotNewLobbyPayload},
    models::game::{
        GameRoomInfo, GameState, GameType, Player, PlayerState, RoomPool, RoomPoolInput,
    },
    state::RedisClient,
};

pub async fn create_room(
    name: String,
    description: Option<String>,
    creator_id: Uuid,
    game_id: Uuid,
    game_name: String,
    pool: Option<RoomPoolInput>,
    redis: RedisClient,
    bot: Bot,
) -> Result<Uuid, AppError> {
    let room_id = Uuid::new_v4();
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let room_info = GameRoomInfo {
        id: room_id,
        name,
        description,
        creator_id,
        state: GameState::Waiting,
        game_id,
        game_name,
        participants: 1,
        contract_address: pool.as_ref().map(|p| p.contract_address.clone()),
    };

    let creator_user = get_user_by_id(creator_id, redis.clone()).await?;

    if let Some(pool_input) = &pool {
        validate_payment_tx(
            &pool_input.tx_id,
            &creator_user.wallet_address,
            &pool_input.contract_address,
            pool_input.entry_amount,
        )
        .await?;

        let pool_struct = RoomPool {
            entry_amount: pool_input.entry_amount,
            contract_address: pool_input.contract_address.clone(),
            current_amount: pool_input.entry_amount,
        };

        let pool_key = format!("room:{}:pool", room_id);
        let pool_json = serde_json::to_string(&pool_struct)
            .map_err(|e| AppError::Serialization(e.to_string()))?;

        let _: () = conn
            .set(pool_key, pool_json)
            .await
            .map_err(AppError::RedisCommandError)?;
    }

    // Serialize room info
    let room_info_json =
        serde_json::to_string(&room_info).map_err(|e| AppError::Serialization(e.to_string()))?;

    let room_player = Player {
        id: creator_user.id,
        wallet_address: creator_user.wallet_address.clone(),
        display_name: creator_user.display_name.clone(),
        state: PlayerState::Ready,
        used_words: Vec::new(),
        rank: None,
        tx_id: pool.as_ref().map(|p| p.tx_id.to_owned()),
        claim: None,
        prize: None,
    };

    let room_info_key = format!("room:{}:info", room_id);

    let room_player_json =
        serde_json::to_string(&room_player).map_err(|e| AppError::Serialization(e.to_string()))?;

    let _: () = redis::pipe()
        .cmd("SET")
        .arg(&room_info_key)
        .arg(room_info_json)
        .ignore()
        .cmd("SADD")
        .arg(format!("room:{}:players", room_id))
        .arg(room_player_json)
        .cmd("SADD")
        .arg(format!("game:{}:rooms", game_id))
        .arg(room_id.to_string())
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
            room_id,
            room_name: room_info.name.clone(),
            description: room_info.description.clone(),
            game_name: room_info.game_name.clone(),
            game_image,
            contract_address: room_info.contract_address.clone(),
            creator_display_name: creator_user.display_name,
            wallet_address: creator_user.wallet_address,
        };

        let chat_id = std::env::var("TELEGRAM_CHAT_ID")
            .expect("TELEGRAM_CHAT_ID must be set")
            .parse::<i64>()
            .unwrap();

        if let Err(e) = bot::broadcast_lobby_created(&bot, chat_id, payload).await {
            tracing::error!("Failed to broadcast lobby creation: {}", e);
        }
    });

    Ok(room_id)
}
