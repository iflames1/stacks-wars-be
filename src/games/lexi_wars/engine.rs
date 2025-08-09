use axum::extract::ws::Message;
use chrono::Utc;
use futures::StreamExt;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::sleep;

use crate::{
    db::lobby::{
        patch::{update_lexi_wars_player, update_lobby_state},
        put::update_connected_players,
    },
    games::lexi_wars::{
        rules::get_rule_by_index,
        utils::{
            broadcast_to_lobby, broadcast_to_player, broadcast_word_entry_from_player,
            close_connections_for_players, get_next_player_and_wrap,
        },
    },
    models::{
        game::{GameData, LexiWars, LobbyState, Player},
        lexi_wars::{LexiWarsClientMessage, LexiWarsServerMessage, PlayerStanding},
    },
    state::{ConnectionInfoMap, LexiWarsLobbies, RedisClient},
};
use uuid::Uuid;

fn get_prize(lobby: &LexiWars, position: usize) -> Option<f64> {
    if lobby.info.contract_address.is_none() || lobby.info.entry_amount.is_none() {
        return None;
    }

    let entry_amount = lobby.info.entry_amount.unwrap();
    let total_pool = entry_amount * lobby.connected_players_count as f64;

    let prize = match position {
        1 => {
            if lobby.connected_players_count == 2 {
                (total_pool * 70.0) / 100.0
            } else {
                (total_pool * 50.0) / 100.0
            }
        }
        2 => (total_pool * 30.0) / 100.0,
        3 => (total_pool * 20.0) / 100.0,
        _ => 0.0,
    };

    Some(prize)
}

pub fn start_auto_start_timer(
    lobby_id: Uuid,
    lobbies: LexiWarsLobbies,
    connections: ConnectionInfoMap,
    redis: RedisClient,
) {
    tokio::spawn(async move {
        for i in (0..=10).rev() {
            {
                let mut lobbies_guard = lobbies.lock().await;
                if let Some(lobby) = lobbies_guard.get_mut(&lobby_id) {
                    let connected_count = lobby.connected_players.len();
                    let total_players = lobby.players.len();

                    tracing::info!(
                        "Auto-start timer: {}s, connected: {}/{}",
                        i,
                        connected_count,
                        total_players
                    );

                    // If all players are connected, start immediately
                    if connected_count == total_players {
                        tracing::info!("All players connected, starting game immediately");

                        // Set current_turn_id to first connected player
                        if let Some(first_connected) = lobby.connected_players.first() {
                            lobby.current_turn_id = first_connected.id;
                        }

                        // Store the fixed count for prize calculation
                        lobby.connected_players_count = lobby.connected_players.len();

                        // Update connected players in Redis
                        if let Err(e) = update_connected_players(
                            lobby_id,
                            lobby.connected_players.clone(),
                            redis.clone(),
                        )
                        .await
                        {
                            tracing::error!("Failed to update connected players in Redis: {}", e);
                        }

                        lobby.game_started = true; // after update_connected_players

                        let start_msg = LexiWarsServerMessage::Start {
                            time: i,
                            started: true,
                        };
                        broadcast_to_lobby(&start_msg, &lobby, &connections, &redis).await;

                        // Set the initial rule and broadcast it
                        if let Some(rule) = get_rule_by_index(lobby.rule_index, &lobby.rule_context)
                        {
                            lobby.current_rule = Some(rule.description.clone());
                            let rule_msg = LexiWarsServerMessage::Rule {
                                rule: rule.description,
                            };
                            broadcast_to_lobby(&rule_msg, &lobby, &connections, &redis).await;
                        }

                        // Start the first turn
                        if let Some(current_player) = lobby
                            .connected_players
                            .iter()
                            .find(|p| p.id == lobby.current_turn_id)
                        {
                            let turn_msg = LexiWarsServerMessage::Turn {
                                current_turn: current_player.clone(),
                            };
                            broadcast_to_lobby(&turn_msg, &lobby, &connections, &redis).await;

                            // Start the turn timer
                            let words = match &lobby.data {
                                GameData::LexiWar { word_list } => word_list.clone(),
                            };

                            start_turn_timer(
                                lobby.current_turn_id,
                                lobby_id,
                                lobbies.clone(),
                                connections.clone(),
                                words,
                                redis.clone(),
                            );
                        }
                        return;
                    }

                    // Send countdown update
                    let start_msg = LexiWarsServerMessage::Start {
                        time: i,
                        started: false,
                    };
                    broadcast_to_lobby(&start_msg, &lobby, &connections, &redis).await;
                } else {
                    tracing::error!("Room not found during auto-start timer, stopping");
                    return;
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        // Timer expired, check if we have at least 50% of players and minimum 2 players
        let mut lobbies_guard = lobbies.lock().await;
        if let Some(lobby) = lobbies_guard.get_mut(&lobby_id) {
            let connected_count = lobby.connected_players.len();
            let total_players = lobby.players.len();
            let required_players = std::cmp::max(2, (total_players + 1) / 2); // At least 2 players and 50% (rounded up)

            tracing::info!(
                "Auto-start timer expired: connected {}/{}, required: {}",
                connected_count,
                total_players,
                required_players
            );

            if connected_count >= required_players && connected_count > 1 {
                tracing::info!(
                    "Sufficient players connected ({}%), starting game",
                    (connected_count * 100) / total_players
                );

                // Set current_turn_id to first connected player
                if let Some(first_connected) = lobby.connected_players.first() {
                    lobby.current_turn_id = first_connected.id;
                }

                // Store the fixed count for prize calculation
                lobby.connected_players_count = lobby.connected_players.len();

                // Update connected players in Redis
                if let Err(e) = update_connected_players(
                    lobby_id,
                    lobby.connected_players.clone(),
                    redis.clone(),
                )
                .await
                {
                    tracing::error!("Failed to update connected players in Redis: {}", e);
                }

                lobby.game_started = true; // after update_connected_players

                let start_msg = LexiWarsServerMessage::Start {
                    time: 0,
                    started: true,
                };
                broadcast_to_lobby(&start_msg, &lobby, &connections, &redis).await;

                // Set the initial rule and broadcast it
                if let Some(rule) = get_rule_by_index(lobby.rule_index, &lobby.rule_context) {
                    lobby.current_rule = Some(rule.description.clone());
                    let rule_msg = LexiWarsServerMessage::Rule {
                        rule: rule.description,
                    };
                    broadcast_to_lobby(&rule_msg, &lobby, &connections, &redis).await;
                }

                // Start the first turn
                if let Some(current_player) = lobby
                    .connected_players
                    .iter()
                    .find(|p| p.id == lobby.current_turn_id)
                {
                    let turn_msg = LexiWarsServerMessage::Turn {
                        current_turn: current_player.clone(),
                    };
                    broadcast_to_lobby(&turn_msg, &lobby, &connections, &redis).await;

                    // Start the turn timer
                    let words = match &lobby.data {
                        GameData::LexiWar { word_list } => word_list.clone(),
                    };

                    start_turn_timer(
                        lobby.current_turn_id,
                        lobby_id,
                        lobbies.clone(),
                        connections.clone(),
                        words,
                        redis.clone(),
                    );
                }
            } else {
                tracing::info!(
                    "Insufficient players connected or less than 2 players, returning lobby to waiting state"
                );

                let start_failed_msg = LexiWarsServerMessage::StartFailed;
                broadcast_to_lobby(&start_failed_msg, &lobby, &connections, &redis).await;

                // Reset lobby state
                lobby.game_started = false;
                lobby.connected_players.clear();

                if let Err(e) =
                    update_lobby_state(lobby_id, LobbyState::Waiting, redis.clone()).await
                {
                    tracing::error!("Error updating game state to Waiting: {}", e);
                }
            }
        }
    });
}

fn start_turn_timer(
    player_id: Uuid,
    lobby_id: Uuid,
    lobbies: LexiWarsLobbies,
    connections: ConnectionInfoMap,
    words: Arc<HashSet<String>>,
    redis: RedisClient,
) {
    tokio::spawn(async move {
        for i in (0..=30).rev() {
            {
                let lobbies_guard = lobbies.lock().await;
                if let Some(lobby) = lobbies_guard.get(&lobby_id) {
                    if lobby.current_turn_id != player_id {
                        let countdown_msg = LexiWarsServerMessage::Countdown { time: 30 };
                        broadcast_to_player(
                            player_id,
                            lobby_id,
                            &countdown_msg,
                            &connections,
                            &redis,
                        )
                        .await;

                        tracing::info!("turn changed, stopping timer");
                        return;
                    }
                    let countdown_msg = LexiWarsServerMessage::Countdown { time: i };
                    broadcast_to_player(player_id, lobby_id, &countdown_msg, &connections, &redis)
                        .await;
                } else {
                    tracing::error!("lobby not found, stopping timer");
                    return;
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        // time ran out
        let mut lobbies_guard = lobbies.lock().await;
        if let Some(lobby) = lobbies_guard.get_mut(&lobby_id) {
            if lobby.current_turn_id == player_id {
                tracing::info!("Player {} timed out", player_id);

                let next_player_id = get_next_player_and_wrap(lobby, player_id);

                if let Some(pos) = lobby
                    .connected_players
                    .iter()
                    .position(|p| p.id == player_id)
                {
                    let player = lobby.connected_players.remove(pos);
                    lobby.eliminated_players.push(player.clone());

                    let position = lobby.connected_players.len() + 1;

                    let rank_msg = LexiWarsServerMessage::Rank {
                        rank: position.to_string(),
                    };
                    broadcast_to_player(player_id, lobby_id, &rank_msg, &connections, &redis).await;

                    let player_used_words = lobby.used_words.remove(&player.id).unwrap_or_default();

                    let prize = get_prize(lobby, position);

                    if let Err(e) = update_lexi_wars_player(
                        lobby_id,
                        player_id,
                        position,
                        prize,
                        player_used_words,
                        redis.clone(),
                    )
                    .await
                    {
                        tracing::error!("Error updating player in Redis: {}", e);
                    }

                    if let Some(amount) = prize {
                        let prize_msg = LexiWarsServerMessage::Prize { amount };
                        broadcast_to_player(player_id, lobby_id, &prize_msg, &connections, &redis)
                            .await;
                    }
                }

                // check game over
                if lobby.connected_players.len() == 1 {
                    // Check if game is already finished to prevent double execution
                    if lobby.info.state == LobbyState::Finished {
                        tracing::warn!("Game already finished for lobby {}", lobby_id);
                        return;
                    }

                    let winner = lobby.connected_players.remove(0);

                    let rank_msg = LexiWarsServerMessage::Rank {
                        rank: "1".to_string(),
                    };
                    broadcast_to_player(winner.id, lobby_id, &rank_msg, &connections, &redis).await;

                    let player_used_words = lobby.used_words.remove(&winner.id).unwrap_or_default();

                    let prize = get_prize(lobby, 1);

                    if let Err(e) = update_lexi_wars_player(
                        lobby_id,
                        winner.id,
                        1,
                        prize,
                        player_used_words,
                        redis.clone(),
                    )
                    .await
                    {
                        tracing::error!("Error updating player in Redis: {}", e);
                    }

                    if let Some(amount) = prize {
                        let prize_msg = LexiWarsServerMessage::Prize { amount };
                        broadcast_to_player(winner.id, lobby_id, &prize_msg, &connections, &redis)
                            .await;
                    }

                    // Move winner to eliminated_players
                    lobby.eliminated_players.push(winner.clone());

                    // Update game state first to prevent race conditions
                    lobby.info.state = LobbyState::Finished;

                    // Send game over messages
                    let gameover_msg = LexiWarsServerMessage::GameOver;
                    broadcast_to_lobby(&gameover_msg, &lobby, &connections, &redis).await;

                    let standing: Vec<PlayerStanding> = lobby
                        .eliminated_players
                        .iter()
                        .rev()
                        .enumerate()
                        .map(|(index, player)| PlayerStanding {
                            player: player.clone(),
                            rank: index + 1,
                        })
                        .collect();

                    let final_standing_msg = LexiWarsServerMessage::FinalStanding { standing };
                    broadcast_to_lobby(&final_standing_msg, &lobby, &connections, &redis).await;

                    if let Err(e) =
                        update_lobby_state(lobby_id, LobbyState::Finished, redis.clone()).await
                    {
                        tracing::error!("Error updating game state in Redis: {}", e);
                    }

                    // Close all player connections since the game has ended
                    let all_player_ids: Vec<Uuid> =
                        lobby.eliminated_players.iter().map(|p| p.id).collect();
                    tracing::info!(
                        "Game finished for lobby {}, closing {} connections",
                        lobby_id,
                        all_player_ids.len()
                    );
                    close_connections_for_players(&all_player_ids, &connections).await;

                    return;
                }
                if lobby.connected_players.is_empty() {
                    tracing::warn!("fix: lobby {} is now empty", lobby.info.id); // never really gets here
                    return;
                }

                //continue game if connected_players > 1
                if let Some(next_id) = next_player_id {
                    lobby.current_turn_id = next_id;

                    if let Some(current_player) =
                        lobby.connected_players.iter().find(|p| p.id == next_id)
                    {
                        let next_turn_msg = LexiWarsServerMessage::Turn {
                            current_turn: current_player.clone(),
                        };
                        broadcast_to_lobby(&next_turn_msg, &lobby, &connections, &redis).await;
                    }

                    start_turn_timer(
                        next_id,
                        lobby_id,
                        lobbies.clone(),
                        connections.clone(),
                        words.clone(),
                        redis.clone(),
                    );
                } else {
                    tracing::warn!("No next player found in lobby {}", lobby.info.id);
                }
            }
        }
    });
}

pub async fn handle_incoming_messages(
    player: &Player,
    lobby_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    lobbies: LexiWarsLobbies,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    while let Some(msg_result) = receiver.next().await {
        match msg_result {
            Ok(msg) => match msg {
                Message::Text(text) => {
                    let parsed = match serde_json::from_str::<LexiWarsClientMessage>(&text) {
                        Ok(msg) => msg,
                        Err(e) => {
                            tracing::info!("Invalid message format: {}", e);
                            continue;
                        }
                    };

                    match parsed {
                        LexiWarsClientMessage::Ping { ts } => {
                            let now = Utc::now().timestamp_millis() as u64;
                            let pong = now.saturating_sub(ts);

                            let msg = LexiWarsServerMessage::Pong { ts, pong };
                            broadcast_to_player(player.id, lobby_id, &msg, connections, &redis)
                                .await
                        }
                        LexiWarsClientMessage::WordEntry { word } => {
                            // Check if game has started before processing word entries
                            {
                                let lobbies_guard = lobbies.lock().await;
                                if let Some(lobby) = lobbies_guard.get(&lobby_id) {
                                    if !lobby.game_started {
                                        tracing::info!(
                                            "Game not started yet, ignoring word entry from {}",
                                            player.wallet_address
                                        );
                                        continue;
                                    }
                                } else {
                                    tracing::error!("Room not found for word entry");
                                    continue;
                                }
                            }

                            let cleaned_word = word.trim().to_lowercase();
                            let advance_turn: bool;

                            {
                                let mut lobbies_guard = lobbies.lock().await;
                                let lobby = lobbies_guard.get_mut(&lobby_id).unwrap();
                                let words = match &lobby.data {
                                    GameData::LexiWar { word_list } => word_list.clone(),
                                };

                                // check turn
                                if player.id != lobby.current_turn_id {
                                    tracing::info!("Not {}'s turn", player.wallet_address);
                                    continue;
                                }

                                // check if word is used
                                if lobby.used_words_in_lobby.contains(&cleaned_word) {
                                    tracing::info!("This word have been used: {}", cleaned_word);
                                    let used_word_msg = LexiWarsServerMessage::UsedWord {
                                        word: cleaned_word.clone(),
                                    };
                                    broadcast_to_player(
                                        player.id,
                                        lobby_id,
                                        &used_word_msg,
                                        connections,
                                        &redis,
                                    )
                                    .await;
                                    continue;
                                }

                                // apply rule
                                if let Some(rule) =
                                    get_rule_by_index(lobby.rule_index, &lobby.rule_context)
                                {
                                    // Update current rule
                                    lobby.current_rule = Some(rule.description.clone());

                                    // untested check
                                    if rule.name != "min_length" {
                                        if cleaned_word.len() < lobby.rule_context.min_word_length {
                                            let reason = format!(
                                                "Word must be at least {} characters!",
                                                lobby.rule_context.min_word_length
                                            );
                                            tracing::info!("Rule failed: {}", reason);
                                            let validation_msg =
                                                LexiWarsServerMessage::Validate { msg: reason };
                                            broadcast_to_player(
                                                player.id,
                                                lobby_id,
                                                &validation_msg,
                                                connections,
                                                &redis,
                                            )
                                            .await;
                                            continue;
                                        }
                                    }
                                    let rule_msg = LexiWarsServerMessage::Rule {
                                        rule: rule.description,
                                    };
                                    broadcast_to_lobby(&rule_msg, &lobby, &connections, &redis)
                                        .await;
                                    if let Err(reason) =
                                        (rule.validate)(&cleaned_word, &lobby.rule_context)
                                    {
                                        tracing::info!("Rule failed: {}", reason);
                                        let validation_msg =
                                            LexiWarsServerMessage::Validate { msg: reason };
                                        broadcast_to_player(
                                            player.id,
                                            lobby_id,
                                            &validation_msg,
                                            connections,
                                            &redis,
                                        )
                                        .await;
                                        continue;
                                    }
                                } else {
                                    tracing::error!("fix invalid rule index {}", lobby.rule_index);
                                }

                                // check if word is valid
                                if !words.contains(&cleaned_word) {
                                    let validation_msg = LexiWarsServerMessage::Validate {
                                        msg: "Invalid word".to_string(),
                                    };
                                    broadcast_to_player(
                                        player.id,
                                        lobby_id,
                                        &validation_msg,
                                        connections,
                                        &redis,
                                    )
                                    .await;
                                    tracing::info!(
                                        "invalid word from {}: {}",
                                        player.wallet_address,
                                        cleaned_word
                                    );
                                    continue;
                                }

                                // add to used words
                                lobby.used_words_in_lobby.insert(cleaned_word.clone());
                                lobby
                                    .used_words
                                    .entry(player.id)
                                    .or_default()
                                    .push(cleaned_word.clone());

                                // store next player id
                                if let Some(next_id) = get_next_player_and_wrap(lobby, player.id) {
                                    lobby.current_turn_id = next_id;

                                    // Update current rule for next turn
                                    if let Some(rule) =
                                        get_rule_by_index(lobby.rule_index, &lobby.rule_context)
                                    {
                                        lobby.current_rule = Some(rule.description.clone());
                                    }

                                    if let Some(current_player) =
                                        lobby.players.iter().find(|p| p.id == next_id)
                                    {
                                        let next_turn_msg = LexiWarsServerMessage::Turn {
                                            current_turn: current_player.clone(),
                                        };
                                        broadcast_to_lobby(
                                            &next_turn_msg,
                                            &lobby,
                                            &connections,
                                            &redis,
                                        )
                                        .await;
                                    }
                                } else {
                                    tracing::error!("couldn't find next player");
                                };

                                // start game loop
                                start_turn_timer(
                                    lobby.current_turn_id,
                                    lobby_id,
                                    lobbies.clone(),
                                    connections.clone(),
                                    words.clone(),
                                    redis.clone(),
                                );

                                advance_turn = true;
                            }
                            if advance_turn {
                                let lobby_guard = lobbies.lock().await;
                                let lobby = lobby_guard.get(&lobby_id).unwrap();
                                broadcast_word_entry_from_player(
                                    player,
                                    &cleaned_word,
                                    &lobby,
                                    &connections,
                                    &redis,
                                )
                                .await;
                            }
                        }
                    }
                }
                Message::Ping(data) => {
                    tracing::debug!("Received ping from player {}", player.id);
                    // Handle WebSocket ping with custom timestamp logic
                    if data.len() == 8 {
                        // Extract timestamp from ping data (8 bytes for u64)
                        let mut ts_bytes = [0u8; 8];
                        if data.len() >= 8 {
                            ts_bytes.copy_from_slice(&data[..8]);
                        }
                        let client_ts = u64::from_le_bytes(ts_bytes);

                        let now = Utc::now().timestamp_millis() as u64;
                        let pong_time = now.saturating_sub(client_ts);

                        // Send as LexiWarsServerMessage::Pong JSON instead of WebSocket pong
                        let pong_msg = LexiWarsServerMessage::Pong {
                            ts: client_ts,
                            pong: pong_time,
                        };
                        broadcast_to_player(player.id, lobby_id, &pong_msg, &connections, &redis)
                            .await;
                    } else {
                        // For standard WebSocket pings without timestamp, use current time
                        let now = Utc::now().timestamp_millis() as u64;
                        let pong_msg = LexiWarsServerMessage::Pong { ts: now, pong: 0 };
                        broadcast_to_player(player.id, lobby_id, &pong_msg, &connections, &redis)
                            .await;
                    }
                }
                Message::Pong(_) => {
                    // Client responded to our ping (if we were sending any)
                    tracing::debug!("Received pong from player {}", player.id);
                }
                Message::Close(_) => {
                    tracing::info!("Player {} closed connection", player.wallet_address);
                    break;
                }
                _ => {}
            },
            Err(e) => {
                tracing::error!(
                    "WebSocket error for player {}: {}",
                    player.wallet_address,
                    e
                );
                break;
            }
        }
    }
}
