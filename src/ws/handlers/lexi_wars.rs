use axum::{
    extract::{ConnectInfo, Path, Query, State, WebSocketUpgrade, ws::WebSocket},
    http::StatusCode,
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use uuid::Uuid;

use crate::{
    db::{
        game::state::{
            get_current_rule, get_current_turn, get_game_started, get_rule_context,
            set_current_turn, set_rule_context, set_rule_index,
        },
        lobby::{
            get::{get_connected_players_ids, get_lobby_info, get_lobby_players},
            patch::{add_connected_player, remove_connected_player},
        },
    },
    errors::AppError,
    games::lexi_wars::{
        self,
        engine::start_auto_start_timer,
        rules::RuleContext,
        utils::{broadcast_to_player, generate_random_letter},
    },
    models::{
        game::{ClaimState, LobbyInfo, LobbyState, Player, PlayerState, WsQueryParams},
        lexi_wars::{LexiWarsServerMessage, PlayerStanding},
    },
    state::{AppState, ConnectionInfoMap, RedisClient},
    ws::handlers::utils::{remove_connection, store_connection_and_send_queued_messages},
};

pub async fn lexi_wars_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    Path(lobby_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::debug!("New Lexi-Wars WebSocket connection from {}", addr);

    let player_id = query.user_id;
    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let bot = state.bot.clone();

    let lobby = get_lobby_info(lobby_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    // Check lobby state
    if lobby.state != LobbyState::InProgress {
        if lobby.state == LobbyState::Finished {
            tracing::info!("Player {} trying to connect to finished game", player_id);

            // Send game over info and close connection
            return Ok(ws.on_upgrade(move |mut socket| async move {
                // Send GameOver message first
                let game_over_msg = LexiWarsServerMessage::GameOver;
                let serialized = serde_json::to_string(&game_over_msg).unwrap();
                let _ = socket
                    .send(axum::extract::ws::Message::Text(serialized.into()))
                    .await;

                // Get lobby data for final standing
                if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
                    // Create final standing from players (they should have ranks)
                    let mut players_with_ranks: Vec<_> =
                        players.into_iter().filter(|p| p.rank.is_some()).collect();

                    // Sort by rank (1st place first)
                    players_with_ranks.sort_by_key(|p| p.rank.unwrap());

                    let standing: Vec<PlayerStanding> = players_with_ranks
                        .clone()
                        .into_iter()
                        .map(|player| PlayerStanding {
                            rank: player.rank.unwrap(),
                            player,
                        })
                        .collect();

                    let final_standing_msg = LexiWarsServerMessage::FinalStanding { standing };
                    let serialized = serde_json::to_string(&final_standing_msg).unwrap();
                    let _ = socket
                        .send(axum::extract::ws::Message::Text(serialized.into()))
                        .await;

                    // Send prize message if player has one AND hasn't claimed it yet
                    if let Some(connecting_player) =
                        players_with_ranks.iter().find(|p| p.id == player_id)
                    {
                        if let Some(prize_amount) = connecting_player.prize {
                            // Check if player has not claimed the prize
                            let should_send_prize = match &connecting_player.claim {
                                Some(ClaimState::NotClaimed) => true,
                                None => false,
                                Some(ClaimState::Claimed { .. }) => false,
                            };

                            if should_send_prize {
                                let prize_msg = LexiWarsServerMessage::Prize {
                                    amount: prize_amount,
                                };
                                let serialized = serde_json::to_string(&prize_msg).unwrap();
                                let _ = socket
                                    .send(axum::extract::ws::Message::Text(serialized.into()))
                                    .await;
                            } else {
                                let prize_msg = LexiWarsServerMessage::Prize { amount: 0.0 };
                                let serialized = serde_json::to_string(&prize_msg).unwrap();
                                let _ = socket
                                    .send(axum::extract::ws::Message::Text(serialized.into()))
                                    .await;
                            }
                        }
                    }
                }

                let _ = socket.close().await;
            }));
        } else if lobby.state == LobbyState::Waiting {
            tracing::debug!("Player {} trying to connect to waiting lobby", player_id);

            // Send StartFailed message and close connection
            return Ok(ws.on_upgrade(move |mut socket| async move {
                let start_failed_msg = LexiWarsServerMessage::StartFailed;
                let serialized = serde_json::to_string(&start_failed_msg).unwrap();
                let _ = socket
                    .send(axum::extract::ws::Message::Text(serialized.into()))
                    .await;
                let _ = socket.close().await;
            }));
        } else {
            tracing::error!("lobby {} has unexpected state: {:?}", lobby_id, lobby.state);
            return Err(AppError::BadRequest("lobby has unexpected state".into()).to_response());
        }
    }

    let players = get_lobby_players(lobby_id, None, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    let players_clone = players.clone();

    let matched_player = players
        .into_iter()
        .find(|p| p.id == player_id && p.state == PlayerState::Ready)
        .ok_or_else(|| {
            AppError::Unauthorized("Player not found or not ready in lobby".into()).to_response()
        })?;

    tracing::info!(
        "Player {} allowed to join lexi wars {}",
        matched_player.id,
        lobby_id
    );

    let is_game_started = get_game_started(lobby_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    let connected_player_ids = get_connected_players_ids(lobby_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    if is_game_started {
        let is_reconnecting = connected_player_ids.contains(&player_id);

        if !is_reconnecting {
            tracing::info!(
                "Player {} trying to join after game started - not an initial player",
                player_id
            );

            // Send AlreadyStarted message and close connection
            return Ok(ws.on_upgrade(move |mut socket| async move {
                let already_started_msg = LexiWarsServerMessage::AlreadyStarted;
                let serialized = serde_json::to_string(&already_started_msg).unwrap();
                let _ = socket
                    .send(axum::extract::ws::Message::Text(serialized.into()))
                    .await;
                let _ = socket.close().await;
            }));
        }

        tracing::info!("Player {} reconnecting to started game", player_id);
    }

    Ok(ws.on_upgrade(move |socket| {
        let lobby_info = lobby.clone();
        handle_lexi_wars_socket(
            socket,
            lobby_id,
            matched_player,
            players_clone,
            connected_player_ids,
            connections,
            lobby_info,
            redis,
            is_game_started,
            bot.clone(),
        )
    }))
}

async fn handle_lexi_wars_socket(
    socket: WebSocket,
    lobby_id: Uuid,
    player: Player,
    players: Vec<Player>,
    connected_player_ids: Vec<Uuid>,
    connections: ConnectionInfoMap,
    lobby_info: LobbyInfo,
    redis: RedisClient,
    is_reconnecting_to_started_game: bool,
    bot: teloxide::Bot,
) {
    let (sender, receiver) = socket.split();

    store_connection_and_send_queued_messages(player.id, lobby_id, sender, &connections, &redis)
        .await;

    setup_player_and_lobby(
        &player,
        lobby_info,
        players,
        connected_player_ids,
        &connections,
        &redis,
        &bot,
    )
    .await;

    // Send reconnection state if joining an already started game
    if is_reconnecting_to_started_game {
        // Send current turn if available
        if let Ok(Some(current_turn_id)) = get_current_turn(lobby_id, redis.clone()).await {
            if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
                if let Some(current_player) = players.iter().find(|p| p.id == current_turn_id) {
                    let turn_msg = LexiWarsServerMessage::Turn {
                        current_turn: current_player.clone(),
                        countdown: 15, // Default countdown for reconnection
                    };
                    broadcast_to_player(player.id, lobby_id, &turn_msg, &connections, &redis).await;
                }
            }
        }

        // Send current rule if available
        if let Ok(Some(current_rule)) = get_current_rule(lobby_id, redis.clone()).await {
            let rule_msg = LexiWarsServerMessage::Rule { rule: current_rule };
            broadcast_to_player(player.id, lobby_id, &rule_msg, &connections, &redis).await;
        }

        tracing::debug!("Sent reconnection state to player {}", player.id);
    }

    lexi_wars::engine::handle_incoming_messages(
        &player,
        lobby_id,
        receiver,
        &connections,
        redis.clone(),
        bot.clone(),
    )
    .await;

    // Handle player disconnection
    let game_started = get_game_started(lobby_id, redis.clone())
        .await
        .unwrap_or(false);
    if !game_started {
        // If game hasn't started, remove from connected players
        if let Err(e) = remove_connected_player(lobby_id, player.id, redis.clone()).await {
            tracing::error!("Failed to remove disconnected player: {}", e);
        }

        tracing::info!(
            "Player {} disconnected from lobby {} (pre-game)",
            player.id,
            lobby_id
        );
    } else {
        tracing::info!(
            "Player {} disconnected from lobby {} (during game). Keeping in connected_player_ids for game continuity.",
            player.id,
            lobby_id
        );
    }

    remove_connection(player.id, &connections).await;
}

async fn setup_player_and_lobby(
    player: &Player,
    lobby_info: LobbyInfo,
    players: Vec<Player>,
    connected_player_ids: Vec<Uuid>,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
    telegram_bot: &teloxide::Bot,
) {
    let lobby_id = lobby_info.id;

    // Initialize game state if not exists
    let game_started = get_game_started(lobby_id, redis.clone())
        .await
        .unwrap_or(false);

    if !game_started {
        // Initialize rule context if not set
        if get_rule_context(lobby_id, redis.clone())
            .await
            .unwrap_or(None)
            .is_none()
        {
            let rule_context = RuleContext {
                min_word_length: 4,
                random_letter: generate_random_letter(),
            };
            let _ = set_rule_context(lobby_id, &rule_context, redis.clone()).await;
            let _ = set_rule_index(lobby_id, 0, redis.clone()).await;
        }

        // Set initial current turn if not set
        if get_current_turn(lobby_id, redis.clone())
            .await
            .unwrap_or(None)
            .is_none()
        {
            let _ = set_current_turn(lobby_id, lobby_info.creator.id, redis.clone()).await;
        }
    }

    // Track connected player by adding to Redis connected players set
    if !connected_player_ids.contains(&player.id) {
        // Add this player to connected players
        if let Err(e) = add_connected_player(lobby_id, player.id, redis.clone()).await {
            tracing::error!("Failed to add connected player: {}", e);
        }
    }

    // Get updated count for logging and auto-start check
    let updated_connected_count = connected_player_ids.len()
        + if connected_player_ids.contains(&player.id) {
            0
        } else {
            1
        };

    tracing::info!(
        "Player {} connected to lobby {}. Connected: {}/{}",
        player.id,
        lobby_id,
        updated_connected_count,
        players.len()
    );

    // Start auto-start timer when first player connects and game hasn't started
    if updated_connected_count == 1 && !game_started {
        tracing::info!(
            "First player connected, starting auto-start timer for lobby {}",
            lobby_id
        );
        start_auto_start_timer(
            lobby_id,
            connections.clone(),
            redis.clone(),
            telegram_bot.clone(),
        );
    }
}
