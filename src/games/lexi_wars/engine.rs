use axum::extract::ws::Message;
use chrono::Utc;
use futures::StreamExt;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::time::sleep;

use crate::{
    db::{
        game_room::{get_game_room, update_game_room},
        room::update_room_player_after_game,
        update_game_state,
    },
    games::lexi_wars::{
        rules::get_rule_by_index,
        utils::{
            broadcast_to_player, broadcast_to_room, broadcast_word_entry_from_player,
            get_next_player_and_wrap,
        },
    },
    models::game::{
        GameData, GameRoom, GameState, LexiWarsClientMessage, LexiWarsServerMessage, Player,
        PlayerStanding,
    },
    state::{ConnectionInfoMap, RedisClient},
};
use uuid::Uuid;

fn get_prize(room: &mut GameRoom, position: usize) -> Option<f64> {
    let prize = room.pool.as_ref().map(|pool| match position {
        1 => {
            if room.eliminated_players.len() == 2 {
                (pool.current_amount * 70.0) / 100.0
            } else {
                (pool.current_amount * 50.0) / 100.0
            }
        }
        2 => (pool.current_amount * 30.0) / 100.0,
        3 => (pool.current_amount * 20.0) / 100.0,
        _ => 0.0,
    });

    prize
}

fn start_turn_timer(
    player_id: Uuid,
    room_id: Uuid,
    connections: ConnectionInfoMap,
    words: Arc<HashSet<String>>,
    redis: RedisClient,
) {
    tokio::spawn(async move {
        for i in (0..=30).rev() {
            // Check if room still exists and turn hasn't changed
            match get_game_room(room_id, redis.clone()).await {
                Ok(room) => {
                    if room.current_turn_id != player_id {
                        let countdown_msg = LexiWarsServerMessage::Countdown { time: 10 };
                        broadcast_to_player(player_id, &countdown_msg, &connections, &redis).await;
                        tracing::info!("turn changed, stopping timer");
                        return;
                    }
                    let countdown_msg = LexiWarsServerMessage::Countdown { time: i };
                    broadcast_to_player(player_id, &countdown_msg, &connections, &redis).await;
                }
                Err(_) => {
                    tracing::error!("room not found, stopping timer");
                    return;
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        // Time ran out - update room in Redis
        match update_game_room(room_id, redis.clone(), |room| {
            if room.current_turn_id == player_id {
                tracing::info!("Player {} timed out", player_id);

                let next_player_id = get_next_player_and_wrap(room, player_id);

                // Remove player from room and save position
                if let Some(pos) = room.players.iter().position(|p| p.id == player_id) {
                    let player = room.players.remove(pos);
                    room.eliminated_players.push(player.clone());

                    // Update current turn to next player if available
                    if let Some(next_id) = next_player_id {
                        room.current_turn_id = next_id;
                    }
                }
            }
        })
        .await
        {
            Ok(room) => {
                // Handle player elimination logic
                let eliminated_player = room.eliminated_players.iter().find(|p| p.id == player_id);
                if let Some(player) = eliminated_player {
                    let position = room.players.len() + 1;

                    let rank_msg = LexiWarsServerMessage::Rank {
                        rank: position.to_string(),
                    };
                    broadcast_to_player(player_id, &rank_msg, &connections, &redis).await;

                    let player_used_words =
                        room.used_words.get(&player.id).cloned().unwrap_or_default();
                    let prize = get_prize(&mut room.clone(), position);

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

                // Check game over
                if room.players.len() == 1 {
                    handle_game_over(room_id, &room, &connections, redis.clone()).await;
                } else if !room.players.is_empty() {
                    // Continue game with next player
                    if let Some(current_player) =
                        room.players.iter().find(|p| p.id == room.current_turn_id)
                    {
                        let next_turn_msg = LexiWarsServerMessage::Turn {
                            current_turn: current_player.clone(),
                        };
                        broadcast_to_room(&next_turn_msg, &room, &connections, &redis).await;
                    }

                    start_turn_timer(
                        room.current_turn_id,
                        room_id,
                        connections.clone(),
                        words.clone(),
                        redis.clone(),
                    );
                }
            }
            Err(e) => {
                tracing::error!("Failed to update room after timeout: {}", e);
            }
        }
    });
}

async fn handle_game_over(
    room_id: Uuid,
    room: &GameRoom,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    if let Some(winner) = room.players.first() {
        let player_used_words = room.used_words.get(&winner.id).cloned().unwrap_or_default();
        let prize = get_prize(&mut room.clone(), 1);

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
            tracing::error!("Error updating winner in Redis: {}", e);
        }

        if let Some(amount) = prize {
            if amount > 0.0 {
                let prize_msg = LexiWarsServerMessage::Prize { amount };
                broadcast_to_player(winner.id, &prize_msg, connections, &redis).await;
            }
        }
    }

    let gameover_msg = LexiWarsServerMessage::GameOver;
    broadcast_to_room(&gameover_msg, &room, connections, &redis).await;

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
    broadcast_to_room(&final_standing_msg, &room, connections, &redis).await;

    if let Err(e) = update_game_state(room_id, GameState::Finished, redis.clone()).await {
        tracing::error!("Error updating game state in Redis: {}", e);
    }
}

pub async fn handle_incoming_messages(
    player: &Player,
    room_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
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
                            let cleaned_word = word.trim().to_lowercase();

                            // Get current room state
                            let room = match get_game_room(room_id, redis.clone()).await {
                                Ok(room) => room,
                                Err(_) => {
                                    tracing::error!("Room {} not found", room_id);
                                    continue;
                                }
                            };

                            let words = match &room.data {
                                GameData::LexiWar { word_list } => word_list.clone(),
                            };

                            // Check turn
                            if player.id != room.current_turn_id {
                                tracing::info!("Not {}'s turn", player.wallet_address);
                                continue;
                            }

                            // Check if word is used
                            if room.used_words_global.contains(&cleaned_word) {
                                tracing::info!("This word have been used: {}", cleaned_word);
                                let used_word_msg = LexiWarsServerMessage::UsedWord {
                                    word: cleaned_word.clone(),
                                };
                                broadcast_to_player(player.id, &used_word_msg, connections, &redis)
                                    .await;
                                continue;
                            }

                            // Apply rule validation
                            if let Some(rule) =
                                get_rule_by_index(room.rule_index, &room.rule_context)
                            {
                                if rule.name != "min_length" {
                                    if cleaned_word.len() < room.rule_context.min_word_length {
                                        let reason = format!(
                                            "Word must be at least {} characters!",
                                            room.rule_context.min_word_length
                                        );
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
                                broadcast_to_room(&rule_msg, &room, connections, &redis).await;

                                if let Err(reason) =
                                    (rule.validate)(&cleaned_word, &room.rule_context)
                                {
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

                            // Check if word is valid
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
                                continue;
                            }

                            // Update room with new word and next turn
                            match update_game_room(room_id, redis.clone(), |room| {
                                // Add to used words
                                room.used_words_global.insert(cleaned_word.clone());
                                room.used_words
                                    .entry(player.id)
                                    .or_default()
                                    .push(cleaned_word.clone());

                                // Get next player
                                if let Some(next_id) = get_next_player_and_wrap(room, player.id) {
                                    room.current_turn_id = next_id;
                                }
                            })
                            .await
                            {
                                Ok(updated_room) => {
                                    // Broadcast word entry
                                    broadcast_word_entry_from_player(
                                        player,
                                        &cleaned_word,
                                        &updated_room,
                                        connections,
                                        &redis,
                                    )
                                    .await;

                                    // Broadcast next turn
                                    if let Some(current_player) = updated_room
                                        .players
                                        .iter()
                                        .find(|p| p.id == updated_room.current_turn_id)
                                    {
                                        let next_turn_msg = LexiWarsServerMessage::Turn {
                                            current_turn: current_player.clone(),
                                        };
                                        broadcast_to_room(
                                            &next_turn_msg,
                                            &updated_room,
                                            connections,
                                            &redis,
                                        )
                                        .await;
                                    }

                                    // Start turn timer
                                    start_turn_timer(
                                        updated_room.current_turn_id,
                                        room_id,
                                        connections.clone(),
                                        words.clone(),
                                        redis.clone(),
                                    );
                                }
                                Err(e) => {
                                    tracing::error!("Failed to update room: {}", e);
                                }
                            }
                        }
                    }
                }
                Message::Ping(data) => {
                    if data.len() == 8 {
                        let mut ts_bytes = [0u8; 8];
                        ts_bytes.copy_from_slice(&data[..8]);
                        let client_ts = u64::from_le_bytes(ts_bytes);

                        let now = Utc::now().timestamp_millis() as u64;
                        let pong_time = now.saturating_sub(client_ts);

                        let pong_msg = LexiWarsServerMessage::Pong {
                            ts: client_ts,
                            pong: pong_time,
                        };
                        broadcast_to_player(player.id, &pong_msg, connections, &redis).await;
                    } else {
                        let now = Utc::now().timestamp_millis() as u64;
                        let pong_msg = LexiWarsServerMessage::Pong { ts: now, pong: 0 };
                        broadcast_to_player(player.id, &pong_msg, connections, &redis).await;
                    }
                }
                Message::Pong(_) => {
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
