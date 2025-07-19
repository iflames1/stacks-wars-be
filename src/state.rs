use axum::extract::ws::{Message, WebSocket};
use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use futures::stream::SplitSink;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::SystemTime,
};
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
}

use crate::models::{game::GameRoom, lobby::JoinRequest};

pub type SharedRooms = Arc<Mutex<HashMap<Uuid, GameRoom>>>;

pub type RedisClient = Pool<RedisConnectionManager>;

pub type LobbyJoinRequests = Arc<Mutex<HashMap<Uuid, Vec<JoinRequest>>>>;

#[derive(Debug, Clone)]
pub struct QueuedMessage {
    pub content: String,
    pub timestamp: SystemTime,
    pub message_type: String,
}

#[derive(Debug)]
pub struct ConnectionInfo {
    pub sender: Arc<Mutex<SplitSink<WebSocket, Message>>>,
    pub message_queue: Arc<Mutex<VecDeque<QueuedMessage>>>,
}

pub type ConnectionInfoMap = Arc<Mutex<HashMap<Uuid, Arc<ConnectionInfo>>>>;
