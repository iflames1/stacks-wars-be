use chrono::Utc;
use teloxide::Bot;
use uuid::Uuid;

use crate::{
    db::{
        game::{get::get_game, patch::update_game_active_lobby},
        tx::validate_payment_tx,
        user::get::get_user_by_id,
    },
    errors::AppError,
    http::bot::{self, BotNewLobbyPayload},
    models::{
        game::{LobbyInfo, LobbyPoolInput, LobbyState, Player},
        redis::{KeyPart, RedisKey},
    },
    state::RedisClient,
};

pub async fn create_lobby(
    name: String,
    description: Option<String>,
    creator_id: Uuid,
    game_id: Uuid,
    pool: Option<LobbyPoolInput>,
    redis: RedisClient,
    bot: Bot,
) -> Result<Uuid, AppError> {
    let lobby_id = Uuid::new_v4();
    let (creator_user, game) = tokio::try_join!(
        get_user_by_id(creator_id, redis.clone()),
        get_game(game_id, redis.clone())
    )?;
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let lobby_info = LobbyInfo {
        id: lobby_id,
        name,
        description,
        creator: creator_user.clone(),
        state: LobbyState::Waiting,
        game: game.clone(),
        participants: 1,
        contract_address: pool.as_ref().map(|p| p.contract_address.clone()),
        created_at: Utc::now(),
        entry_amount: pool.as_ref().map(|p| p.entry_amount),
        current_amount: pool.as_ref().map(|p| p.current_amount),
    };

    // Store pool if it exists
    if let Some(pool_input) = &pool {
        validate_payment_tx(
            &pool_input.tx_id,
            &creator_user.wallet_address,
            &pool_input.contract_address,
            pool_input.current_amount,
        )
        .await?;
    }

    let lobby_key = RedisKey::lobby(KeyPart::Id(lobby_id));
    let player_key = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Id(creator_user.id));

    // Create player with minimal data
    let lobby_player = Player::new(creator_user.id, pool.as_ref().map(|p| p.tx_id.to_owned()));
    let player_hash = lobby_player.to_redis_hash();
    let lobby_fields = lobby_info.to_redis_hash();
    let created_score = lobby_info.created_at.timestamp();

    // Rest of the function remains the same...
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
        .arg(RedisKey::lobbies_state(&LobbyState::Waiting))
        .arg(created_score)
        .arg(lobby_id.to_string())
        .ignore()
        .cmd("ZADD")
        .arg(RedisKey::game_lobbies(KeyPart::Id(game_id)))
        .arg(created_score)
        .arg(lobby_id.to_string())
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    update_game_active_lobby(game_id, true, redis.clone()).await?;

    // Bot notification code remains the same...
    tokio::spawn(async move {
        let payload = BotNewLobbyPayload {
            lobby_id,
            lobby_name: lobby_info.name.clone(),
            description: lobby_info.description.clone(),
            game: lobby_info.game,
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
