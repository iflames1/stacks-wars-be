use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StacksSweeperCell {
    pub x: usize,
    pub y: usize,
    pub is_mine: bool,
    pub adjacent: u8,
    pub revealed: bool,
    pub flagged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StacksSweeperGame {
    pub id: Uuid,
    pub user_id: Uuid,
    pub size: usize,
    pub risk: f32,
    pub cells: Vec<StacksSweeperCell>,
    pub game_state: GameState,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub first_move: bool,
    pub blind: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GameState {
    Playing,
    Won,
    Lost,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StacksSweeperClientMessage {
    CellReveal { x: usize, y: usize },
    CellFlag { x: usize, y: usize },
    Ping { ts: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StacksSweeperServerMessage {
    #[serde(rename_all = "camelCase")]
    GameBoard {
        cells: Vec<MaskedCell>,
        game_state: GameState,
        time_remaining: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    GameOver {
        won: bool,
        cells: Vec<MaskedCell>, // Unmasked board
    },
    #[serde(rename_all = "camelCase")]
    Countdown {
        time_remaining: u64,
    },
    #[serde(rename_all = "camelCase")]
    TimeUp {
        cells: Vec<MaskedCell>, // Unmasked board
    },
    Pong {
        ts: u64,
        pong: u64,
    },
    Error {
        message: String,
    },
}

impl StacksSweeperServerMessage {
    pub fn should_queue(&self) -> bool {
        match self {
            // Time-sensitive messages that should NOT be queued
            StacksSweeperServerMessage::Countdown { .. } => false,
            StacksSweeperServerMessage::Pong { .. } => false,

            // Important messages that SHOULD be queued
            StacksSweeperServerMessage::GameBoard { .. } => true,
            StacksSweeperServerMessage::GameOver { .. } => true,
            StacksSweeperServerMessage::TimeUp { .. } => true,
            StacksSweeperServerMessage::Error { .. } => true,
        }
    }
}

impl StacksSweeperGame {
    pub fn new(user_id: Uuid, size: usize, risk: f32, cells: Vec<StacksSweeperCell>) -> Self {
        Self {
            id: Uuid::new_v4(),
            user_id,
            size,
            risk,
            cells,
            game_state: GameState::Playing,
            created_at: chrono::Utc::now(),
            first_move: true,
            blind: false, // Default to false, will be set by caller
        }
    }

    pub fn to_redis_hash(&self) -> HashMap<String, String> {
        let mut hash = HashMap::new();
        hash.insert("id".to_string(), self.id.to_string());
        hash.insert("user_id".to_string(), self.user_id.to_string());
        hash.insert("size".to_string(), self.size.to_string());
        hash.insert("risk".to_string(), self.risk.to_string());
        hash.insert(
            "cells".to_string(),
            serde_json::to_string(&self.cells).unwrap_or_default(),
        );
        hash.insert(
            "game_state".to_string(),
            serde_json::to_string(&self.game_state).unwrap_or_default(),
        );
        hash.insert("created_at".to_string(), self.created_at.to_rfc3339());
        hash.insert("first_move".to_string(), self.first_move.to_string());
        hash.insert("blind".to_string(), self.blind.to_string());
        hash
    }

    pub fn from_redis_hash(
        hash: HashMap<String, String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            id: Uuid::parse_str(&hash.get("id").ok_or("Missing id")?)?,
            user_id: Uuid::parse_str(&hash.get("user_id").ok_or("Missing user_id")?)?,
            size: hash.get("size").ok_or("Missing size")?.parse()?,
            risk: hash.get("risk").ok_or("Missing risk")?.parse()?,
            cells: serde_json::from_str(hash.get("cells").ok_or("Missing cells")?)?,
            game_state: serde_json::from_str(hash.get("game_state").ok_or("Missing game_state")?)?,
            created_at: chrono::DateTime::parse_from_rfc3339(
                hash.get("created_at").ok_or("Missing created_at")?,
            )?
            .into(),
            first_move: hash
                .get("first_move")
                .ok_or("Missing first_move")?
                .parse()?,
            blind: hash.get("blind").ok_or("Missing blind")?.parse()?,
        })
    }

    // Get masked cells (only showing revealed information)
    pub fn get_masked_cells(&self) -> Vec<MaskedCell> {
        self.cells
            .iter()
            .map(|cell| MaskedCell {
                x: cell.x,
                y: cell.y,
                revealed: cell.revealed,
                flagged: cell.flagged,
                adjacent: if cell.revealed && !cell.is_mine {
                    if self.blind {
                        None // In blind mode, never show adjacent count
                    } else {
                        Some(cell.adjacent) // In normal mode, show adjacent count
                    }
                } else {
                    None
                },
                is_mine: if cell.revealed && cell.is_mine {
                    Some(true)
                } else {
                    None
                },
            })
            .collect()
    }

    // Shift a mine from the given position to a random safe position
    pub fn shift_mine(&mut self, x: usize, y: usize) -> Result<(), String> {
        // Find the cell at the given position
        let cell_index = y * self.size + x;
        if cell_index >= self.cells.len() {
            return Err("Invalid cell position".to_string());
        }

        // Check if the cell is actually a mine
        if !self.cells[cell_index].is_mine {
            return Err("Cell is not a mine".to_string());
        }

        // Find all non-mine cells that can become mines
        let safe_positions: Vec<usize> = self
            .cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| !cell.is_mine)
            .map(|(index, _)| index)
            .collect();

        if safe_positions.is_empty() {
            return Err("No safe positions available to move mine".to_string());
        }

        // Choose a random safe position
        use rand::Rng;
        let mut rng = rand::rng();
        let random_index = rng.random_range(0..safe_positions.len());
        let new_position = safe_positions[random_index];

        // Move the mine
        self.cells[cell_index].is_mine = false;
        self.cells[new_position].is_mine = true;

        // Recalculate adjacent counts for all cells
        self.recalculate_adjacent_counts();

        Ok(())
    }

    // Recalculate adjacent mine counts for all cells
    fn recalculate_adjacent_counts(&mut self) {
        for y in 0..self.size {
            for x in 0..self.size {
                let cell_index = y * self.size + x;

                if !self.cells[cell_index].is_mine {
                    let mut adjacent = 0;

                    // Check all 8 adjacent cells
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            if dx == 0 && dy == 0 {
                                continue; // Skip the cell itself
                            }

                            let nx = x as isize + dx;
                            let ny = y as isize + dy;

                            if nx >= 0
                                && nx < self.size as isize
                                && ny >= 0
                                && ny < self.size as isize
                            {
                                let neighbor_index = (ny as usize) * self.size + (nx as usize);
                                if self.cells[neighbor_index].is_mine {
                                    adjacent += 1;
                                }
                            }
                        }
                    }

                    self.cells[cell_index].adjacent = adjacent;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaskedCell {
    pub x: usize,
    pub y: usize,
    pub revealed: bool,
    pub flagged: bool,
    pub adjacent: Option<u8>,
    pub is_mine: Option<bool>,
}
