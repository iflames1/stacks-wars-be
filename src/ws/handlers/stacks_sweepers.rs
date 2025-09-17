use axum::{
    extract::{ConnectInfo, Query, State, WebSocketUpgrade, ws::WebSocket},
    http::StatusCode,
    response::IntoResponse,
};
use futures::StreamExt;
use std::net::SocketAddr;
use uuid::Uuid;

use crate::{
    db::lobby::get::{get_lobby_info, get_lobby_players},
    games::stacks_sweepers::{
        engine::{handle_incoming_messages as handle_multiplayer_messages, start_auto_start_timer},
        single::{
            handle_incoming_messages as handle_single_messages, send_existing_game_if_exists,
        },
    },
    models::game::{LobbyState, WsQueryParams},
    state::AppState,
    ws::handlers::utils::store_connection_and_send_queued_messages,
};
use teloxide::Bot;

pub async fn stacks_sweepers_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::debug!("New StacksSweeper WebSocket connection from {}", addr);

    let player_id = query.user_id;
    let lobby_id = query.lobby_id;
    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let telegram_bot = state.bot.clone();

    if let Some(lobby_id) = lobby_id {
        tracing::info!(
            "Player {} connected to StacksSweeper lobby {}",
            player_id,
            lobby_id
        );
    } else {
        tracing::info!(
            "Player {} connected to StacksSweeper single-player",
            player_id
        );
    }

    Ok(ws.on_upgrade(move |socket| {
        handle_stacks_sweeper_socket(
            socket,
            player_id,
            lobby_id,
            connections,
            redis,
            telegram_bot,
        )
    }))
}

async fn handle_stacks_sweeper_socket(
    socket: WebSocket,
    player_id: Uuid,
    lobby_id: Option<Uuid>,
    connections: crate::state::ConnectionInfoMap,
    redis: crate::state::RedisClient,
    telegram_bot: Bot,
) {
    let (sender, receiver) = socket.split();

    match lobby_id {
        Some(lobby_id) => {
            // Multiplayer mode
            tracing::info!(
                "WebSocket established for StacksSweeper multiplayer player {} in lobby {}",
                player_id,
                lobby_id
            );

            // Store connection
            store_connection_and_send_queued_messages(
                player_id,
                lobby_id,
                sender,
                &connections,
                &redis,
            )
            .await;

            // Get lobby info and player data
            let lobby_info = match get_lobby_info(lobby_id, redis.clone()).await {
                Ok(info) => info,
                Err(e) => {
                    tracing::error!("Failed to get lobby info: {}", e);
                    return;
                }
            };

            let players = match get_lobby_players(lobby_id, None, redis.clone()).await {
                Ok(players) => players,
                Err(e) => {
                    tracing::error!("Failed to get lobby players: {}", e);
                    return;
                }
            };

            // Find the player
            let player = match players.iter().find(|p| p.id == player_id) {
                Some(player) => player.clone(),
                None => {
                    tracing::error!("Player {} not found in lobby {}", player_id, lobby_id);
                    return;
                }
            };

            // Check if we should start the auto-start timer
            if lobby_info.state == LobbyState::Waiting {
                // Update lobby state to Starting
                if let Err(e) = crate::db::lobby::patch::update_lobby_state(
                    lobby_id,
                    LobbyState::Starting,
                    redis.clone(),
                )
                .await
                {
                    tracing::error!("Failed to update lobby state to Starting: {}", e);
                    return;
                }

                // Start auto-start timer
                start_auto_start_timer(
                    lobby_id,
                    connections.clone(),
                    redis.clone(),
                    telegram_bot.clone(),
                );
            }

            // Handle multiplayer messages
            handle_multiplayer_messages(
                &player,
                lobby_id,
                receiver,
                &connections,
                redis,
                telegram_bot,
            )
            .await;

            tracing::info!(
                "WebSocket closed for StacksSweeper multiplayer player {} in lobby {}",
                player_id,
                lobby_id
            );
        }
        None => {
            // Single-player mode
            tracing::info!(
                "WebSocket established for StacksSweeper single-player {}",
                player_id
            );

            // Store connection using player_id as lobby_id for single player
            store_connection_and_send_queued_messages(
                player_id,
                player_id, // Using player_id as lobby_id for single player
                sender,
                &connections,
                &redis,
            )
            .await;

            // Send existing game state if player has one
            send_existing_game_if_exists(&player_id, &connections, &redis).await;

            // Handle single-player messages
            handle_single_messages(player_id, receiver, &connections, redis).await;

            tracing::info!(
                "WebSocket closed for StacksSweeper single-player {}",
                player_id
            );
        }
    }
}
