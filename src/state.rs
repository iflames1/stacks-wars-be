use axum::extract::ws::{Message, WebSocket};
use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use futures::stream::SplitSink;
use std::{collections::HashMap, sync::Arc};
use teloxide::Bot;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub rooms: SharedRooms,
    pub connections: ConnectionInfoMap,
    pub redis: RedisClient,
    pub lobby_join_requests: LobbyJoinRequests,
    pub bot: Bot,
    pub lobby_countdowns: LobbyCountdowns,
}

use crate::models::{game::GameRoom, lobby::JoinRequest};

#[derive(Debug)]
pub struct ConnectionInfo {
    pub sender: Arc<Mutex<SplitSink<WebSocket, Message>>>,
}

#[derive(Debug, Clone)]
pub struct CountdownState {
    pub current_time: u32,
}

impl Default for CountdownState {
    fn default() -> Self {
        Self { current_time: 15 }
    }
}

pub type SharedRooms = Arc<Mutex<HashMap<Uuid, GameRoom>>>;

pub type ConnectionInfoMap = Arc<Mutex<HashMap<Uuid, Arc<ConnectionInfo>>>>;

pub type RedisClient = Pool<RedisConnectionManager>;

pub type LobbyJoinRequests = Arc<Mutex<HashMap<Uuid, Vec<JoinRequest>>>>;

pub type LobbyCountdowns = Arc<Mutex<HashMap<Uuid, CountdownState>>>;
