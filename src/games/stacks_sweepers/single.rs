use futures::{SinkExt, StreamExt};
use uuid::Uuid;

use crate::{
    db::stacks_sweeper::countdown::get_stacks_sweeper_countdown,
    db::tx::validate_payment_tx,
    db::user::get::get_user_by_id,
    errors::AppError,
    models::{
        redis::{KeyPart, RedisKey},
        stacks_sweeper::{
            CellState, MaskedCell, StacksSweeperClientMessage, StacksSweeperGame,
            StacksSweeperGameState, StacksSweeperServerMessage, calc_cashout_multiplier,
        },
    },
    state::{ConnectionInfoMap, RedisClient},
    ws::handlers::utils::{queue_message_for_player, remove_connection},
};

use super::{
    // Board, // TODO: Implement Board struct for single-player mode
    handlers::{
        handle_cashout, handle_cell_flag, handle_cell_reveal, handle_multiplier_target, handle_ping,
    },
};

pub fn get_unmasked_cells(game: &StacksSweeperGame) -> Vec<MaskedCell> {
    game.cells
        .iter()
        .map(|cell| MaskedCell {
            x: cell.x,
            y: cell.y,
            state: if cell.is_mine {
                Some(CellState::Mine)
            } else {
                Some(CellState::Adjacent {
                    count: cell.adjacent,
                })
            },
        })
        .collect()
}

pub async fn handle_incoming_messages(
    player_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<axum::extract::ws::Message, axum::Error>> + Unpin,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    while let Some(msg_result) = receiver.next().await {
        match msg_result {
            Ok(msg) => match msg {
                axum::extract::ws::Message::Text(text) => {
                    match serde_json::from_str::<StacksSweeperClientMessage>(&text) {
                        Ok(client_message) => match client_message {
                            StacksSweeperClientMessage::CreateBoard {
                                size,
                                risk,
                                blind,
                                amount,
                                tx_id,
                            } => {
                                handle_create_board(
                                    player_id,
                                    size,
                                    risk,
                                    blind,
                                    amount,
                                    tx_id,
                                    connections,
                                    redis.clone(),
                                )
                                .await;
                            }
                            StacksSweeperClientMessage::MultiplierTarget { size, risk } => {
                                handle_multiplier_target(
                                    player_id,
                                    size,
                                    risk,
                                    connections,
                                    &redis,
                                )
                                .await;
                            }
                            StacksSweeperClientMessage::Cashout { tx_id } => {
                                handle_cashout(player_id, tx_id, connections, redis.clone()).await;
                            }
                            StacksSweeperClientMessage::Ping { ts } => {
                                handle_ping(player_id, ts, connections, &redis).await;
                            }
                            StacksSweeperClientMessage::CellReveal { x, y } => {
                                tracing::info!(
                                    "Player {} revealed cell at ({}, {})",
                                    player_id,
                                    x,
                                    y
                                );
                                handle_cell_reveal(player_id, x, y, connections, redis.clone())
                                    .await;
                            }
                            StacksSweeperClientMessage::CellFlag { x, y } => {
                                tracing::info!(
                                    "Player {} flagged cell at ({}, {})",
                                    player_id,
                                    x,
                                    y
                                );
                                handle_cell_flag(player_id, x, y, connections, redis.clone()).await;
                            }
                        },
                        Err(e) => {
                            tracing::warn!("Failed to parse StacksSweeper message: {}", e);
                        }
                    }
                }
                _ => {}
            },
            Err(e) => {
                tracing::debug!("WebSocket error for player {}: {}", player_id, e);
                break;
            }
        }
    }
}

pub async fn send_existing_game_if_exists(
    player_id: &Uuid,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    match get_game_from_redis(*player_id, redis).await {
        Ok(game) => {
            // Check if game has ended - if so, send unmasked cells with GameOver message
            if matches!(
                game.game_state,
                StacksSweeperGameState::Won | StacksSweeperGameState::Lost
            ) {
                let won = matches!(game.game_state, StacksSweeperGameState::Won);
                let unmasked_cells = get_unmasked_cells(&game);
                let message = StacksSweeperServerMessage::GameOver {
                    won,
                    cells: unmasked_cells,
                    mines: game.get_mine_count(),
                    board_size: game.size,
                };
                tracing::debug!("Sending game over state to reconnected player: {message:?}");
                broadcast_to_player(*player_id, *player_id, &message, connections, redis).await;

                // Send claim info for finished games
                let claim_info_msg = StacksSweeperServerMessage::ClaimInfo {
                    claim_state: game.claim_state.clone(),
                    cashout_amount: game.get_cashout_amount(),
                    current_multiplier: None,
                    revealed_count: None,
                    size: None,
                    risk: None,
                };
                broadcast_to_player(*player_id, *player_id, &claim_info_msg, connections, redis)
                    .await;
            } else {
                // Send existing game board with masked cells for ongoing games
                let masked_cells = game.get_masked_cells();

                // Get actual countdown time from Redis if game is playing
                let time_remaining = if matches!(game.game_state, StacksSweeperGameState::Playing) {
                    get_stacks_sweeper_countdown(*player_id, redis.clone())
                        .await
                        .unwrap_or(None)
                } else {
                    None
                };

                let message = StacksSweeperServerMessage::GameBoard {
                    cells: masked_cells,
                    game_state: game.game_state,
                    time_remaining,
                    mines: game.get_mine_count(),
                    board_size: game.size,
                };
                tracing::debug!("Sending ongoing game state to reconnected player: {message:?}");
                broadcast_to_player(*player_id, *player_id, &message, connections, redis).await;

                // Send claim info for reconnecting player
                let claim_info_msg = StacksSweeperServerMessage::ClaimInfo {
                    claim_state: game.claim_state.clone(),
                    cashout_amount: game.get_cashout_amount(),
                    current_multiplier: if game.user_revealed_count > 0
                        && matches!(game.game_state, StacksSweeperGameState::Playing)
                    {
                        Some(calc_cashout_multiplier(
                            game.size,
                            game.risk as f64,
                            game.user_revealed_count as usize,
                        ))
                    } else {
                        None
                    },
                    revealed_count: if game.user_revealed_count > 0 {
                        Some(game.user_revealed_count as usize)
                    } else {
                        None
                    },
                    size: if game.user_revealed_count > 0 {
                        Some(game.size)
                    } else {
                        None
                    },
                    risk: if game.user_revealed_count > 0 {
                        Some(game.risk)
                    } else {
                        None
                    },
                };
                broadcast_to_player(*player_id, *player_id, &claim_info_msg, connections, redis)
                    .await;
            }
        }
        Err(_) => {
            // No existing game - send NoBoard message
            let no_board_msg = StacksSweeperServerMessage::NoBoard {
                message: "No existing game found. Create a new board to start playing.".to_string(),
            };
            broadcast_to_player(*player_id, *player_id, &no_board_msg, connections, redis).await;
            tracing::info!("No existing game found for player {}", player_id);
        }
    }
}

async fn handle_create_board(
    player_id: Uuid,
    size: usize,
    risk: f32,
    blind: bool,
    amount: f64,
    tx_id: String,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    // Validate input parameters
    if size < 3 || size > 10 {
        let error_msg = StacksSweeperServerMessage::Error {
            message: "Grid size must be between 3 and 10".to_string(),
        };
        broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
        let no_board_msg = StacksSweeperServerMessage::NoBoard {
            message: "Error creating board.".to_string(),
        };
        broadcast_to_player(player_id, player_id, &no_board_msg, connections, &redis).await;
        return;
    }
    if risk < 0.1 || risk > 0.9 {
        let error_msg = StacksSweeperServerMessage::Error {
            message: "Risk must be between 0.1 and 0.9".to_string(),
        };
        broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
        let no_board_msg = StacksSweeperServerMessage::NoBoard {
            message: "Error creating board.".to_string(),
        };
        broadcast_to_player(player_id, player_id, &no_board_msg, connections, &redis).await;
        return;
    }

    // Validate payment transaction
    if amount > 0.0 {
        let expected_contract = "STF0V8KWBS70F0WDKTMY65B3G591NN52PR4Z71Y3.stacks-sweepers-pool-v1";

        // Get the user's wallet address
        match get_user_by_id(player_id, redis.clone()).await {
            Ok(user) => {
                match validate_payment_tx(&tx_id, &user.wallet_address, expected_contract, amount)
                    .await
                {
                    Ok(()) => {
                        tracing::info!(
                            "Payment validated for player {}: amount {}, tx_id {}",
                            player_id,
                            amount,
                            tx_id
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "Payment validation failed for player {}: {}",
                            player_id,
                            e
                        );
                        let error_msg = StacksSweeperServerMessage::Error {
                            message: format!("Payment validation failed: {}", e),
                        };
                        broadcast_to_player(player_id, player_id, &error_msg, connections, &redis)
                            .await;
                        let no_board_msg = StacksSweeperServerMessage::NoBoard {
                            message: "Payment validation failed.".to_string(),
                        };
                        broadcast_to_player(
                            player_id,
                            player_id,
                            &no_board_msg,
                            connections,
                            &redis,
                        )
                        .await;
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to get user data for player {}: {}", player_id, e);
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Failed to verify user data".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                let no_board_msg = StacksSweeperServerMessage::NoBoard {
                    message: "User verification failed.".to_string(),
                };
                broadcast_to_player(player_id, player_id, &no_board_msg, connections, &redis).await;
                return;
            }
        }
    }

    // Check if player can create a new game
    if let Ok(existing_game) = get_game_from_redis(player_id, &redis).await {
        if !existing_game.can_create_new() {
            let error_msg = StacksSweeperServerMessage::Error {
                message: "Cannot create a new game while current game is in progress".to_string(),
            };
            broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
            return;
        }
    }

    match create_stacks_sweeper_single(player_id, size, risk, blind, amount, tx_id, redis.clone())
        .await
    {
        Ok(_) => {
            // Get the created game and send it to the player
            match get_game_from_redis(player_id, &redis).await {
                Ok(game) => {
                    let masked_cells = game.get_masked_cells();
                    let created_msg = StacksSweeperServerMessage::BoardCreated {
                        cells: masked_cells,
                        game_state: game.game_state,
                        mines: game.get_mine_count(),
                        board_size: game.size,
                    };
                    broadcast_to_player(player_id, player_id, &created_msg, connections, &redis)
                        .await;
                    tracing::info!("Board created and sent to player {}", player_id);
                }
                Err(e) => {
                    tracing::error!("Failed to get created game for player {}: {}", player_id, e);
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to create game for player {}: {}", player_id, e);
            let error_msg = StacksSweeperServerMessage::Error {
                message: "Failed to create game".to_string(),
            };
            broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
        }
    }
}

pub async fn create_stacks_sweeper_single(
    user_id: Uuid,
    size: usize,
    risk: f32,
    blind: bool,
    amount: f64,
    tx_id: String,
    redis: RedisClient,
) -> Result<Uuid, AppError> {
    // TODO: Implement Board struct for single-player mode
    // Generate the board
    // let board = Board::generate(size, risk);
    // let game_cells = board.to_game_cells();
    let game_cells = vec![]; // Placeholder for single-player mode

    // Create the game instance
    let mut game = StacksSweeperGame::new(
        user_id,
        "Unknown".to_string(),
        size,
        risk,
        game_cells,
        amount,
        tx_id,
    );
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
                .flat_map(|(k, v)| [k.as_str(), v.as_str()])
                .collect::<Vec<_>>(),
        )
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(game_id)
}

pub async fn get_game_from_redis(
    player_id: Uuid,
    redis: &RedisClient,
) -> Result<StacksSweeperGame, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let game_key = RedisKey::stacks_sweeper(KeyPart::Id(player_id));

    let game_hash: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&game_key)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    if game_hash.is_empty() {
        return Err(AppError::NotFound("Game not found".into()));
    }

    StacksSweeperGame::from_redis_hash(game_hash)
        .map_err(|e| AppError::Deserialization(format!("Failed to deserialize game: {}", e)))
}

pub async fn save_game_to_redis(
    game: &StacksSweeperGame,
    redis: &RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let game_key = RedisKey::stacks_sweeper(KeyPart::Id(
        Uuid::parse_str(&game.user_id).unwrap_or(Uuid::new_v4()),
    ));
    let game_hash = game.to_redis_hash();

    let _: () = redis::cmd("HSET")
        .arg(&game_key)
        .arg(
            game_hash
                .iter()
                .flat_map(|(k, v)| [k.as_str(), v.as_str()])
                .collect::<Vec<_>>(),
        )
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

pub async fn broadcast_to_player(
    player_id: Uuid,
    lobby_id: Uuid, // Using same as player_id for single player games
    message: &StacksSweeperServerMessage,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let serialized = match serde_json::to_string(message) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to serialize StacksSweeper message: {}", e);
            return;
        }
    };

    // Try to send directly to connected player
    {
        let connections_guard = connections.lock().await;
        if let Some(connection_info) = connections_guard.get(&player_id) {
            let mut sender = connection_info.sender.lock().await;
            if let Err(e) = sender
                .send(axum::extract::ws::Message::Text(serialized.clone().into()))
                .await
            {
                tracing::debug!("Failed to send message to player {}: {}", player_id, e);
                drop(sender);
                drop(connections_guard);
                remove_connection(player_id, connections).await;
            } else {
                return;
            }
        }
    }

    // If direct send failed or player not connected, queue the message if it should be queued
    if message.should_queue() {
        if let Err(e) = queue_message_for_player(player_id, lobby_id, serialized, redis).await {
            tracing::error!("Failed to queue message for player {}: {}", player_id, e);
        }
    }
}
