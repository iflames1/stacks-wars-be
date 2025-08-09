use axum::{
    extract::{ConnectInfo, Path, Query, State, WebSocketUpgrade, ws::WebSocket},
    http::StatusCode,
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
};

use crate::{
    db::lobby::get::{get_connected_players, get_lobby_info, get_lobby_players},
    games::lexi_wars::{
        engine::start_auto_start_timer,
        utils::{broadcast_to_player, generate_random_letter},
    },
    models::{
        game::{
            ClaimState, GameData, LexiWars, LobbyInfo, LobbyState, Player, PlayerState,
            WsQueryParams,
        },
        lexi_wars::{LexiWarsServerMessage, PlayerStanding},
        word_loader::WORD_LIST,
    },
    state::{AppState, RedisClient},
    ws::handlers::utils::{remove_connection, store_connection_and_send_queued_messages},
};
use crate::{errors::AppError, games::lexi_wars};
use crate::{
    games::lexi_wars::rules::RuleContext,
    state::{ConnectionInfoMap, LexiWarsLobbies},
};
use uuid::Uuid;

async fn setup_player_and_lobby(
    player: &Player,
    lobby_info: LobbyInfo,
    players: Vec<Player>,
    connected_players: Vec<Player>,
    lobbies: &LexiWarsLobbies,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let mut locked_lobbies = lobbies.lock().await;

    // Check if this lobby is already active in memory
    let lobby = locked_lobbies.entry(lobby_info.id).or_insert_with(|| {
        tracing::info!("Initializing new in-memory LexiWars for {}", lobby_info.id);
        let word_list = WORD_LIST.clone();

        LexiWars {
            info: lobby_info.clone(),
            players: players.clone(),
            data: GameData::LexiWar { word_list },
            eliminated_players: vec![],
            current_turn_id: lobby_info.creator.id,
            used_words: HashMap::new(),
            used_words_in_lobby: HashSet::new(),
            rule_context: RuleContext {
                min_word_length: 4,
                random_letter: generate_random_letter(),
            },
            rule_index: 0,
            connected_players: connected_players.clone(),
            connected_players_count: connected_players.len(),
            game_started: false,
            current_rule: None,
        }
    });

    let already_exists = lobby.players.iter().any(|p| p.id == player.id);

    if !already_exists {
        tracing::info!(
            "Adding player {} ({}) to lobby {}",
            player.wallet_address,
            player.id,
            lobby.info.id
        );
        lobby.players.push(player.clone());
    } else {
        tracing::info!(
            "Player {} already exists in lobby {}, skipping re-add",
            player.wallet_address,
            lobby.info.id
        );
    }

    // Track connected player - add to connected_players if not already there
    let already_connected = lobby.connected_players.iter().any(|p| p.id == player.id);
    let was_empty = lobby.connected_players.is_empty();

    if !already_connected {
        lobby.connected_players.push(player.clone());
    }

    tracing::info!(
        "Player {} connected to lobby {}. Connected: {}/{}",
        player.wallet_address,
        lobby.info.id,
        lobby.connected_players.len(),
        lobby.players.len()
    );

    // Start auto-start timer when first player connects
    if was_empty && !lobby.game_started {
        tracing::info!(
            "First player connected, starting auto-start timer for lobby {}",
            lobby.info.id
        );
        start_auto_start_timer(
            lobby.info.id,
            lobbies.clone(),
            connections.clone(),
            redis.clone(),
        );
    }
}

pub async fn lexi_wars_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    Path(lobby_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::info!("New Lexi-Wars WebSocket connection from {}", addr);

    let player_id = query.user_id;

    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let lexi_wars_lobbies = state.lexi_wars_lobbies.clone();

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
        } else {
            tracing::error!("lobby {} is still in waiting", lobby_id);
            return Err(AppError::BadRequest("lobby is still in waiting".into()).to_response());
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
        "Player {} allowed to join lobby {}",
        matched_player.wallet_address,
        lobby_id
    );

    let is_game_started = {
        let lobbies_guard = lexi_wars_lobbies.lock().await;
        if let Some(game_lobby) = lobbies_guard.get(&lobby_id) {
            game_lobby.game_started
        } else {
            false // If not in memory, game hasn't started yet
        }
    };

    let connected_players = get_connected_players(lobby_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    if is_game_started {
        let is_reconnecting = connected_players.iter().any(|p| p.id == player_id);

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
            connected_players,
            lexi_wars_lobbies,
            connections,
            lobby_info,
            redis,
            is_game_started,
        )
    }))
}

async fn handle_lexi_wars_socket(
    socket: WebSocket,
    lobby_id: Uuid,
    player: Player,
    players: Vec<Player>,
    connected_players: Vec<Player>,
    lexi_wars_lobbies: LexiWarsLobbies,
    connections: ConnectionInfoMap,
    lobby_info: LobbyInfo,
    redis: RedisClient,
    is_reconnecting_to_started_game: bool,
) {
    let (sender, receiver) = socket.split();

    store_connection_and_send_queued_messages(player.id, lobby_id, sender, &connections, &redis)
        .await;

    setup_player_and_lobby(
        &player,
        lobby_info,
        players,
        connected_players,
        &lexi_wars_lobbies,
        &connections,
        &redis,
    )
    .await;

    // Send reconnection state if joining an already started game
    if is_reconnecting_to_started_game {
        let lobbies_guard = lexi_wars_lobbies.lock().await;
        if let Some(lobby) = lobbies_guard.get(&lobby_id) {
            // Send current turn
            if let Some(current_player) =
                lobby.players.iter().find(|p| p.id == lobby.current_turn_id)
            {
                let turn_msg = LexiWarsServerMessage::Turn {
                    current_turn: current_player.clone(),
                };
                broadcast_to_player(player.id, lobby_id, &turn_msg, &connections, &redis).await;
            }

            // Send current rule
            if let Some(current_rule) = &lobby.current_rule {
                let rule_msg = LexiWarsServerMessage::Rule {
                    rule: current_rule.clone(),
                };
                broadcast_to_player(player.id, lobby_id, &rule_msg, &connections, &redis).await;
            }

            tracing::info!(
                "Sent reconnection state to player {}",
                player.wallet_address
            );
        }
    }

    lexi_wars::engine::handle_incoming_messages(
        &player,
        lobby_id,
        receiver,
        lexi_wars_lobbies.clone(),
        &connections,
        redis,
    )
    .await;

    // Handle player disconnection
    {
        let mut lobbies_guard = lexi_wars_lobbies.lock().await;
        if let Some(lobby) = lobbies_guard.get_mut(&lobby_id) {
            if let Some(pos) = lobby
                .connected_players
                .iter()
                .position(|p| p.id == player.id)
            {
                // Only remove from connected_players if game hasn't started
                if !lobby.game_started {
                    lobby.connected_players.remove(pos);
                    tracing::info!(
                        "Player {} disconnected from lobby {} (pre-game). Connected: {}/{}",
                        player.wallet_address,
                        lobby_id,
                        lobby.connected_players.len(),
                        lobby.players.len()
                    );

                    // If the disconnected player was the current turn, assign to next connected player
                    if lobby.current_turn_id == player.id && !lobby.connected_players.is_empty() {
                        lobby.current_turn_id = lobby.connected_players[0].id;
                        tracing::info!(
                            "Reassigned current turn to player {}",
                            lobby.current_turn_id
                        );
                    }
                } else {
                    tracing::info!(
                        "Player {} disconnected from lobby {} (during game). Keeping in connected_players for game continuity.",
                        player.wallet_address,
                        lobby_id
                    );
                }
            }
        }
    }

    remove_connection(player.id, &connections).await;
}
