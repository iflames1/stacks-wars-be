use axum::extract::ws::Message;
use chrono::Utc;
use futures::StreamExt;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::sleep;

use crate::{
    db::{
        room::{put::update_connected_players, update_room_player_after_game},
        update_game_state,
    },
    games::lexi_wars::{
        rules::get_rule_by_index,
        utils::{
            broadcast_to_player, broadcast_to_room, broadcast_word_entry_from_player,
            close_connections_for_players, get_next_player_and_wrap,
        },
    },
    models::{
        game::{GameData, GameRoom, GameState, Player},
        lexi_wars::{LexiWarsClientMessage, LexiWarsServerMessage, PlayerStanding},
    },
    state::{ConnectionInfoMap, RedisClient, SharedRooms},
};
use uuid::Uuid;

fn get_prize(room: &GameRoom, position: usize) -> Option<f64> {
    let prize = room.pool.as_ref().map(|pool| {
        // Use the fixed connected_players_count instead of current connected_players.len()
        let total_pool = pool.entry_amount * room.connected_players_count as f64;
        match position {
            1 => {
                if room.connected_players_count == 2 {
                    (total_pool * 70.0) / 100.0
                } else {
                    (total_pool * 50.0) / 100.0
                }
            }
            2 => (total_pool * 30.0) / 100.0,
            3 => (total_pool * 20.0) / 100.0,
            _ => 0.0,
        }
    });

    prize
}

pub fn start_auto_start_timer(
    room_id: Uuid,
    rooms: SharedRooms,
    connections: ConnectionInfoMap,
    redis: RedisClient,
) {
    tokio::spawn(async move {
        for i in (0..=10).rev() {
            {
                let mut rooms_guard = rooms.lock().await;
                if let Some(room) = rooms_guard.get_mut(&room_id) {
                    let connected_count = room.connected_players.len();
                    let total_players = room.players.len();

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
                        if let Some(first_connected) = room.connected_players.first() {
                            room.current_turn_id = first_connected.id;
                        }

                        // Store the fixed count for prize calculation
                        room.connected_players_count = room.connected_players.len();

                        // Update connected players in Redis
                        if let Err(e) = update_connected_players(
                            room_id,
                            room.connected_players.clone(),
                            redis.clone(),
                        )
                        .await
                        {
                            tracing::error!("Failed to update connected players in Redis: {}", e);
                        }

                        room.game_started = true; // after update_connected_players

                        let start_msg = LexiWarsServerMessage::Start {
                            time: i,
                            started: true,
                        };
                        broadcast_to_room(&start_msg, &room, &connections, &redis).await;

                        // Set the initial rule and broadcast it
                        if let Some(rule) = get_rule_by_index(room.rule_index, &room.rule_context) {
                            room.current_rule = Some(rule.description.clone());
                            let rule_msg = LexiWarsServerMessage::Rule {
                                rule: rule.description,
                            };
                            broadcast_to_room(&rule_msg, &room, &connections, &redis).await;
                        }

                        // Start the first turn
                        if let Some(current_player) = room
                            .connected_players
                            .iter()
                            .find(|p| p.id == room.current_turn_id)
                        {
                            let turn_msg = LexiWarsServerMessage::Turn {
                                current_turn: current_player.clone(),
                            };
                            broadcast_to_room(&turn_msg, &room, &connections, &redis).await;

                            // Start the turn timer
                            let words = match &room.data {
                                GameData::LexiWar { word_list } => word_list.clone(),
                            };

                            start_turn_timer(
                                room.current_turn_id,
                                room_id,
                                rooms.clone(),
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
                    broadcast_to_room(&start_msg, &room, &connections, &redis).await;
                } else {
                    tracing::error!("Room not found during auto-start timer, stopping");
                    return;
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        // Timer expired, check if we have at least 50% of players and minimum 2 players
        let mut rooms_guard = rooms.lock().await;
        if let Some(room) = rooms_guard.get_mut(&room_id) {
            let connected_count = room.connected_players.len();
            let total_players = room.players.len();
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
                if let Some(first_connected) = room.connected_players.first() {
                    room.current_turn_id = first_connected.id;
                }

                // Store the fixed count for prize calculation
                room.connected_players_count = room.connected_players.len();

                // Update connected players in Redis
                if let Err(e) =
                    update_connected_players(room_id, room.connected_players.clone(), redis.clone())
                        .await
                {
                    tracing::error!("Failed to update connected players in Redis: {}", e);
                }

                room.game_started = true; // after update_connected_players

                let start_msg = LexiWarsServerMessage::Start {
                    time: 0,
                    started: true,
                };
                broadcast_to_room(&start_msg, &room, &connections, &redis).await;

                // Set the initial rule and broadcast it
                if let Some(rule) = get_rule_by_index(room.rule_index, &room.rule_context) {
                    room.current_rule = Some(rule.description.clone());
                    let rule_msg = LexiWarsServerMessage::Rule {
                        rule: rule.description,
                    };
                    broadcast_to_room(&rule_msg, &room, &connections, &redis).await;
                }

                // Start the first turn
                if let Some(current_player) = room
                    .connected_players
                    .iter()
                    .find(|p| p.id == room.current_turn_id)
                {
                    let turn_msg = LexiWarsServerMessage::Turn {
                        current_turn: current_player.clone(),
                    };
                    broadcast_to_room(&turn_msg, &room, &connections, &redis).await;

                    // Start the turn timer
                    let words = match &room.data {
                        GameData::LexiWar { word_list } => word_list.clone(),
                    };

                    start_turn_timer(
                        room.current_turn_id,
                        room_id,
                        rooms.clone(),
                        connections.clone(),
                        words,
                        redis.clone(),
                    );
                }
            } else {
                tracing::info!(
                    "Insufficient players connected or less than 2 players, returning room to waiting state"
                );

                let start_failed_msg = LexiWarsServerMessage::StartFailed;
                broadcast_to_room(&start_failed_msg, &room, &connections, &redis).await;

                // Reset room state
                room.game_started = false;
                room.connected_players.clear();

                if let Err(e) = update_game_state(room_id, GameState::Waiting, redis.clone()).await
                {
                    tracing::error!("Error updating game state to Waiting: {}", e);
                }
            }
        }
    });
}

fn start_turn_timer(
    player_id: Uuid,
    room_id: Uuid,
    rooms: SharedRooms,
    connections: ConnectionInfoMap,
    words: Arc<HashSet<String>>,
    redis: RedisClient,
) {
    tokio::spawn(async move {
        for i in (0..=30).rev() {
            {
                let rooms_guard = rooms.lock().await;
                if let Some(room) = rooms_guard.get(&room_id) {
                    if room.current_turn_id != player_id {
                        let countdown_msg = LexiWarsServerMessage::Countdown { time: 30 };
                        broadcast_to_player(player_id, &countdown_msg, &connections, &redis).await;

                        tracing::info!("turn changed, stopping timer");
                        return;
                    }
                    let countdown_msg = LexiWarsServerMessage::Countdown { time: i };
                    broadcast_to_player(player_id, &countdown_msg, &connections, &redis).await;
                } else {
                    tracing::error!("room not found, stopping timer");
                    return;
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        // time ran out
        let mut rooms_guard = rooms.lock().await;
        if let Some(room) = rooms_guard.get_mut(&room_id) {
            if room.current_turn_id == player_id {
                tracing::info!("Player {} timed out", player_id);

                let next_player_id = get_next_player_and_wrap(room, player_id);

                if let Some(pos) = room
                    .connected_players
                    .iter()
                    .position(|p| p.id == player_id)
                {
                    let player = room.connected_players.remove(pos);
                    room.eliminated_players.push(player.clone());

                    let position = room.connected_players.len() + 1;

                    let rank_msg = LexiWarsServerMessage::Rank {
                        rank: position.to_string(),
                    };
                    broadcast_to_player(player_id, &rank_msg, &connections, &redis).await;

                    let player_used_words = room.used_words.remove(&player.id).unwrap_or_default();

                    let prize = get_prize(room, position);

                    if let Err(e) = update_room_player_after_game(
                        room_id,
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
                        if amount > 0.0 {
                            let prize_msg = LexiWarsServerMessage::Prize { amount };
                            broadcast_to_player(player_id, &prize_msg, &connections, &redis).await;
                        }
                    }
                }

                // check game over
                if room.connected_players.len() == 1 {
                    // Check if game is already finished to prevent double execution
                    if room.info.state == GameState::Finished {
                        tracing::warn!("Game already finished for room {}", room_id);
                        return;
                    }

                    let winner = room.connected_players.remove(0);

                    let rank_msg = LexiWarsServerMessage::Rank {
                        rank: "1".to_string(),
                    };
                    broadcast_to_player(winner.id, &rank_msg, &connections, &redis).await;

                    let player_used_words = room.used_words.remove(&winner.id).unwrap_or_default();

                    let prize = get_prize(room, 1);

                    if let Err(e) = update_room_player_after_game(
                        room_id,
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
                        if amount > 0.0 {
                            let prize_msg = LexiWarsServerMessage::Prize { amount };
                            broadcast_to_player(winner.id, &prize_msg, &connections, &redis).await;
                        }
                    }

                    // Move winner to eliminated_players
                    room.eliminated_players.push(winner.clone());

                    // Update game state first to prevent race conditions
                    room.info.state = GameState::Finished;

                    // Send game over messages
                    let gameover_msg = LexiWarsServerMessage::GameOver;
                    broadcast_to_room(&gameover_msg, &room, &connections, &redis).await;

                    let standing: Vec<PlayerStanding> = room
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
                    broadcast_to_room(&final_standing_msg, &room, &connections, &redis).await;

                    if let Err(e) =
                        update_game_state(room_id, GameState::Finished, redis.clone()).await
                    {
                        tracing::error!("Error updating game state in Redis: {}", e);
                    }

                    // Close all player connections since the game has ended
                    let all_player_ids: Vec<Uuid> =
                        room.eliminated_players.iter().map(|p| p.id).collect();
                    tracing::info!(
                        "Game finished for room {}, closing {} connections",
                        room_id,
                        all_player_ids.len()
                    );
                    close_connections_for_players(&all_player_ids, &connections).await;

                    return;
                }
                if room.connected_players.is_empty() {
                    tracing::warn!("fix: room {} is now empty", room.info.id); // never really gets here
                    return;
                }

                //continue game if connected_players > 1
                if let Some(next_id) = next_player_id {
                    room.current_turn_id = next_id;

                    if let Some(current_player) =
                        room.connected_players.iter().find(|p| p.id == next_id)
                    {
                        let next_turn_msg = LexiWarsServerMessage::Turn {
                            current_turn: current_player.clone(),
                        };
                        broadcast_to_room(&next_turn_msg, &room, &connections, &redis).await;
                    }

                    start_turn_timer(
                        next_id,
                        room_id,
                        rooms.clone(),
                        connections.clone(),
                        words.clone(),
                        redis.clone(),
                    );
                } else {
                    tracing::warn!("No next player found in room {}", room.info.id);
                }
            }
        }
    });
}

pub async fn handle_incoming_messages(
    player: &Player,
    room_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    rooms: SharedRooms,
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
                            broadcast_to_player(player.id, &msg, connections, &redis).await
                        }
                        LexiWarsClientMessage::WordEntry { word } => {
                            // Check if game has started before processing word entries
                            {
                                let rooms_guard = rooms.lock().await;
                                if let Some(room) = rooms_guard.get(&room_id) {
                                    if !room.game_started {
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
                                let mut rooms_guard = rooms.lock().await;
                                let room = rooms_guard.get_mut(&room_id).unwrap();
                                let words = match &room.data {
                                    GameData::LexiWar { word_list } => word_list.clone(),
                                };

                                // check turn
                                if player.id != room.current_turn_id {
                                    tracing::info!("Not {}'s turn", player.wallet_address);
                                    continue;
                                }

                                // check if word is used
                                if room.used_words_global.contains(&cleaned_word) {
                                    tracing::info!("This word have been used: {}", cleaned_word);
                                    let used_word_msg = LexiWarsServerMessage::UsedWord {
                                        word: cleaned_word.clone(),
                                    };
                                    broadcast_to_player(
                                        player.id,
                                        &used_word_msg,
                                        connections,
                                        &redis,
                                    )
                                    .await;
                                    continue;
                                }

                                // apply rule
                                if let Some(rule) =
                                    get_rule_by_index(room.rule_index, &room.rule_context)
                                {
                                    // Update current rule
                                    room.current_rule = Some(rule.description.clone());

                                    // untested check
                                    if rule.name != "min_length" {
                                        if cleaned_word.len() < room.rule_context.min_word_length {
                                            let reason = format!(
                                                "Word must be at least {} characters!",
                                                room.rule_context.min_word_length
                                            );
                                            tracing::info!("Rule failed: {}", reason);
                                            let validation_msg =
                                                LexiWarsServerMessage::Validate { msg: reason };
                                            broadcast_to_player(
                                                player.id,
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
                                    broadcast_to_room(&rule_msg, &room, &connections, &redis).await;
                                    if let Err(reason) =
                                        (rule.validate)(&cleaned_word, &room.rule_context)
                                    {
                                        tracing::info!("Rule failed: {}", reason);
                                        let validation_msg =
                                            LexiWarsServerMessage::Validate { msg: reason };
                                        broadcast_to_player(
                                            player.id,
                                            &validation_msg,
                                            connections,
                                            &redis,
                                        )
                                        .await;
                                        continue;
                                    }
                                } else {
                                    tracing::error!("fix invalid rule index {}", room.rule_index);
                                }

                                // check if word is valid
                                if !words.contains(&cleaned_word) {
                                    let validation_msg = LexiWarsServerMessage::Validate {
                                        msg: "Invalid word".to_string(),
                                    };
                                    broadcast_to_player(
                                        player.id,
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
                                room.used_words_global.insert(cleaned_word.clone());
                                room.used_words
                                    .entry(player.id)
                                    .or_default()
                                    .push(cleaned_word.clone());

                                // store next player id
                                if let Some(next_id) = get_next_player_and_wrap(room, player.id) {
                                    room.current_turn_id = next_id;

                                    // Update current rule for next turn
                                    if let Some(rule) =
                                        get_rule_by_index(room.rule_index, &room.rule_context)
                                    {
                                        room.current_rule = Some(rule.description.clone());
                                    }

                                    if let Some(current_player) =
                                        room.players.iter().find(|p| p.id == next_id)
                                    {
                                        let next_turn_msg = LexiWarsServerMessage::Turn {
                                            current_turn: current_player.clone(),
                                        };
                                        broadcast_to_room(
                                            &next_turn_msg,
                                            &room,
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
                                    room.current_turn_id,
                                    room_id,
                                    rooms.clone(),
                                    connections.clone(),
                                    words.clone(),
                                    redis.clone(),
                                );

                                advance_turn = true;
                            }
                            if advance_turn {
                                let room_gaurd = rooms.lock().await;
                                let room = room_gaurd.get(&room_id).unwrap();
                                broadcast_word_entry_from_player(
                                    player,
                                    &cleaned_word,
                                    &room,
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
                        broadcast_to_player(player.id, &pong_msg, &connections, &redis).await;
                    } else {
                        // For standard WebSocket pings without timestamp, use current time
                        let now = Utc::now().timestamp_millis() as u64;
                        let pong_msg = LexiWarsServerMessage::Pong { ts: now, pong: 0 };
                        broadcast_to_player(player.id, &pong_msg, &connections, &redis).await;
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
