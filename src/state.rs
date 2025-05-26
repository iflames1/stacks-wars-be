use axum::extract::ws::WebSocket;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

use axum::extract::ws::Message;
use futures::stream::SplitSink;

#[derive(Clone)]
pub struct AppState {
    pub rooms: Rooms,
    pub connections: Connections,
}

use crate::models::GameRoom;

pub type Rooms = Arc<Mutex<HashMap<Uuid, GameRoom>>>;

pub type Sender = Arc<Mutex<SplitSink<WebSocket, Message>>>;

pub type Connections = Arc<Mutex<HashMap<Uuid, Sender>>>;
