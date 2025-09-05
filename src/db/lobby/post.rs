use chrono::Utc;
use teloxide::Bot;
use uuid::Uuid;

use crate::{
    db::{
        game::{get::get_game, patch::update_game_active_lobby},
        tx::{validate_fee_transfer, validate_payment_tx},
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
    tx_id: String,
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

    // Create player with minimal data
    let lobby_player = Player::new(creator_user.id, Some(tx_id.clone()));
    let creator_last_ping = lobby_player.last_ping;

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
        token_symbol: pool.as_ref().and_then(|p| p.token_symbol.clone()),
        token_id: pool.as_ref().and_then(|p| p.token_id.clone()),
        creator_last_ping,
        tg_msg_id: None,
    };

    // Store pool if it exists
    if let Some(pool_input) = &pool {
        validate_payment_tx(
            &tx_id,
            &creator_user.wallet_address,
            &pool_input.contract_address,
            pool_input.current_amount,
        )
        .await?;
    } else {
        let fee_wallet = std::env::var("FEE_WALLET")
            .map_err(|_| AppError::EnvError("FEE_WALLET not set".into()))?;

        validate_fee_transfer(&tx_id, &creator_user.wallet_address, &fee_wallet).await?;
    }

    let lobby_key = RedisKey::lobby(KeyPart::Id(lobby_id));
    let player_key = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Id(creator_user.id));

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
        .arg(RedisKey::lobbies_all())
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

    let redis_for_tg = redis.clone();
    tokio::spawn(async move {
        let payload = BotNewLobbyPayload {
            lobby_id,
            lobby_name: lobby_info.name.clone(),
            description: lobby_info.description.clone(),
            game: lobby_info.game,
            entry_amount: pool.as_ref().and_then(|p| Some(p.entry_amount)),
            current_amount: pool.as_ref().map(|p| p.current_amount),
            contract_address: lobby_info.contract_address.clone(),
            token_symbol: pool.as_ref().and_then(|p| p.token_symbol.clone()),
            creator_name: creator_user.display_name.or(creator_user.username),
            wallet_address: creator_user.wallet_address.clone(),
        };

        let chat_id = std::env::var("TELEGRAM_CHAT_ID")
            .expect("TELEGRAM_CHAT_ID must be set")
            .parse::<i64>()
            .unwrap();

        match bot::broadcast_lobby_created(&bot, chat_id, payload).await {
            Ok(msg) => {
                // Store the telegram message ID in Redis
                let lobby_key = RedisKey::lobby(KeyPart::Id(lobby_id));
                if let Ok(mut conn) = redis_for_tg.get().await {
                    let _: Result<(), redis::RedisError> = redis::cmd("HSET")
                        .arg(&lobby_key)
                        .arg("tg_msg_id")
                        .arg(msg.id.0)
                        .query_async(&mut *conn)
                        .await;
                }
            }
            Err(e) => {
                tracing::error!("Failed to broadcast lobby creation: {}", e);
            }
        }
    });

    Ok(lobby_id)
}

pub async fn create_stacks_sweeper_single(
    user_id: Uuid,
    size: usize,
    risk: f32,
    blind: bool,
    redis: RedisClient,
) -> Result<Uuid, AppError> {
    use crate::{games::stacks_sweepers::Board, models::stacks_sweeper::StacksSweeperGame};

    // Validate input parameters
    if size < 3 || size > 10 {
        return Err(AppError::BadRequest(
            "Grid size must be between 3 and 10".into(),
        ));
    }
    if risk < 0.1 || risk > 0.9 {
        return Err(AppError::BadRequest(
            "Risk must be between 0.1 and 0.9".into(),
        ));
    }

    // Generate the board
    let board = Board::generate(size, risk);
    let game_cells = board.to_game_cells();

    // Create the game instance
    let mut game = StacksSweeperGame::new(user_id, size, risk, game_cells);
    game.blind = blind;
    let game_id = game.id;

    // Store in Redis
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let game_key = RedisKey::stacks_sweeper(KeyPart::Id(user_id));
    let game_hash = game.to_redis_hash();

    // Store the game data
    let _: () = redis::cmd("HSET")
        .arg(&game_key)
        .arg(
            game_hash
                .iter()
                .flat_map(|(k, v)| [k.as_ref(), v.as_str()])
                .collect::<Vec<&str>>(),
        )
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    tracing::info!(
        "Created StacksSweeper game {} for user {} with size {}x{} and risk {}",
        game_id,
        user_id,
        size,
        size,
        risk
    );

    Ok(game_id)
}
