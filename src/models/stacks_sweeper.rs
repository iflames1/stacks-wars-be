use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::game::ClaimState;

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
    pub board: Vec<Vec<u8>>,
    pub state: StacksSweeperGameState,
    pub user_id: String,
    pub username: String,
    pub player_board: Vec<Vec<StacksSweeperCellState>>,
    pub countdown: u32,
    pub user_revealed_count: u32,
    pub amount: f64,
    pub tx_id: String,
    pub claim_state: Option<ClaimState>,
    // Additional fields needed by the existing code
    pub size: usize,
    pub risk: f32,
    pub cells: Vec<StacksSweeperCell>,
    pub game_state: StacksSweeperGameState, // Alias for state for backward compatibility
    pub created_at: DateTime<Utc>,
    pub first_move: bool,
    pub blind: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StacksSweeperGameState {
    Waiting,
    Playing,
    Won,
    Lost,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StacksSweeperCellState {
    Hidden,
    Revealed,
    Flagged,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StacksSweeperClientMessage {
    #[serde(rename_all = "camelCase")]
    CreateBoard {
        size: usize,
        risk: f32,
        blind: bool,
        amount: f64,
        tx_id: String,
    },
    CellReveal {
        x: usize,
        y: usize,
    },
    CellFlag {
        x: usize,
        y: usize,
    },
    MultiplierTarget {
        size: usize,
        risk: f32,
    },
    #[serde(rename_all = "camelCase")]
    Cashout {
        tx_id: String,
    },
    Ping {
        ts: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StacksSweeperServerMessage {
    #[serde(rename_all = "camelCase")]
    GameBoard {
        cells: Vec<MaskedCell>,
        game_state: StacksSweeperGameState,
        time_remaining: Option<u64>,
        mines: u32,
        board_size: usize,
    },
    #[serde(rename_all = "camelCase")]
    BoardCreated {
        cells: Vec<MaskedCell>,
        game_state: StacksSweeperGameState,
        mines: u32,
        board_size: usize,
    },
    #[serde(rename_all = "camelCase")]
    NoBoard {
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    GameOver {
        won: bool,
        cells: Vec<MaskedCell>, // Unmasked board
        mines: u32,
        board_size: usize,
    },
    #[serde(rename_all = "camelCase")]
    Countdown {
        time_remaining: u64,
    },
    #[serde(rename_all = "camelCase")]
    TimeUp {
        cells: Vec<MaskedCell>, // Unmasked board
        mines: u32,
        board_size: usize,
    },
    #[serde(rename_all = "camelCase")]
    MultiplierTarget {
        max_multiplier: f64,
        size: usize,
        risk: f32,
    },
    #[serde(rename_all = "camelCase")]
    ClaimInfo {
        claim_state: Option<ClaimState>,
        cashout_amount: Option<f64>,
        current_multiplier: Option<f64>,
        revealed_count: Option<usize>,
        size: Option<usize>,
        risk: Option<f32>,
    },
    Pong {
        ts: u64,
        pong: u64,
    },
    Error {
        message: String,
    },

    // Multiplayer messages
    #[serde(rename_all = "camelCase")]
    Turn {
        current_turn: crate::models::game::Player,
        countdown: u32,
    },
    #[serde(rename_all = "camelCase")]
    CellRevealed {
        x: usize,
        y: usize,
        cell_state: CellState,
        revealed_by: String, // Player username who revealed it
    },
    #[serde(rename_all = "camelCase")]
    Start {
        time: u32,
        started: bool,
    },
    StartFailed,
    #[serde(rename_all = "camelCase")]
    Rank {
        rank: String,
    },
    #[serde(rename_all = "camelCase")]
    Prize {
        amount: f64,
    },
    #[serde(rename_all = "camelCase")]
    WarsPoint {
        wars_point: f64,
    },
    MultiplayerGameOver,
    #[serde(rename_all = "camelCase")]
    FinalStanding {
        standing: Vec<PlayerStanding>,
    },
    #[serde(rename_all = "camelCase")]
    Eliminated {
        player: crate::models::game::Player,
        reason: EliminationReason,
        mine_position: Option<(usize, usize)>, // Position of mine if hit
    },
    AlreadyStarted,
}

impl StacksSweeperServerMessage {
    pub fn should_queue(&self) -> bool {
        match self {
            // Time-sensitive messages that should NOT be queued
            StacksSweeperServerMessage::Countdown { .. } => false,
            StacksSweeperServerMessage::Pong { .. } => false,

            // Important messages that SHOULD be queued
            StacksSweeperServerMessage::GameBoard { .. } => true,
            StacksSweeperServerMessage::BoardCreated { .. } => true,
            StacksSweeperServerMessage::NoBoard { .. } => true,
            StacksSweeperServerMessage::GameOver { .. } => true,
            StacksSweeperServerMessage::TimeUp { .. } => true,
            StacksSweeperServerMessage::MultiplierTarget { .. } => true,
            StacksSweeperServerMessage::ClaimInfo { .. } => true,
            StacksSweeperServerMessage::Error { .. } => true,

            // Multiplayer messages
            StacksSweeperServerMessage::Turn { .. } => true,
            StacksSweeperServerMessage::CellRevealed { .. } => true,
            StacksSweeperServerMessage::Start { .. } => true,
            StacksSweeperServerMessage::StartFailed => true,
            StacksSweeperServerMessage::Rank { .. } => true,
            StacksSweeperServerMessage::Prize { .. } => true,
            StacksSweeperServerMessage::WarsPoint { .. } => true,
            StacksSweeperServerMessage::MultiplayerGameOver => true,
            StacksSweeperServerMessage::FinalStanding { .. } => true,
            StacksSweeperServerMessage::Eliminated { .. } => true,
            StacksSweeperServerMessage::AlreadyStarted { .. } => true,
        }
    }
}

impl StacksSweeperGame {
    pub fn new(
        user_id: Uuid,
        username: String,
        size: usize,
        risk: f32,
        cells: Vec<StacksSweeperCell>,
        amount: f64,
        tx_id: String,
    ) -> Self {
        let player_board = vec![vec![StacksSweeperCellState::Hidden; size]; size];
        let board = vec![vec![0u8; size]; size]; // Initialize empty board

        Self {
            id: Uuid::new_v4(),
            board,
            state: StacksSweeperGameState::Waiting,
            user_id: user_id.to_string(),
            username,
            player_board,
            countdown: 60, // Default 60 seconds
            user_revealed_count: 0,
            amount,
            tx_id,
            claim_state: Some(ClaimState::NotClaimed),
            size,
            risk,
            cells,
            game_state: StacksSweeperGameState::Waiting,
            created_at: chrono::Utc::now(),
            first_move: true,
            blind: false, // Default to false, will be set by caller
        }
    }

    pub fn to_redis_hash(&self) -> HashMap<String, String> {
        let mut hash = HashMap::new();
        hash.insert("id".to_string(), self.id.to_string());
        hash.insert("user_id".to_string(), self.user_id.to_string());
        hash.insert("username".to_string(), self.username.clone());
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
        hash.insert(
            "user_revealed_count".to_string(),
            self.user_revealed_count.to_string(),
        );
        hash.insert("countdown".to_string(), self.countdown.to_string());
        hash.insert("amount".to_string(), self.amount.to_string());
        hash.insert("tx_id".to_string(), self.tx_id.clone());
        if let Some(ref claim_state) = self.claim_state {
            hash.insert(
                "claim_state".to_string(),
                serde_json::to_string(claim_state).unwrap_or_default(),
            );
        } else {
            hash.insert("claim_state".to_string(), "".to_string());
        }
        hash
    }

    pub fn from_redis_hash(
        hash: HashMap<String, String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let size: usize = hash.get("size").ok_or("Missing size")?.parse()?;
        let player_board = vec![vec![StacksSweeperCellState::Hidden; size]; size];
        let board = vec![vec![0u8; size]; size]; // Initialize empty board

        Ok(Self {
            id: Uuid::parse_str(&hash.get("id").ok_or("Missing id")?)?,
            board,
            state: serde_json::from_str(hash.get("game_state").ok_or("Missing game_state")?)?,
            user_id: hash.get("user_id").ok_or("Missing user_id")?.clone(),
            username: hash
                .get("username")
                .unwrap_or(&"Unknown".to_string())
                .clone(),
            player_board,
            countdown: hash
                .get("countdown")
                .unwrap_or(&"60".to_string())
                .parse()
                .unwrap_or(60),
            user_revealed_count: hash
                .get("user_revealed_count")
                .ok_or("Missing user_revealed_count")?
                .parse()?,
            amount: hash
                .get("amount")
                .unwrap_or(&"0.0".to_string())
                .parse()
                .unwrap_or(0.0),
            tx_id: hash.get("tx_id").unwrap_or(&"".to_string()).clone(),
            claim_state: hash.get("claim_state").and_then(|s| {
                if s.is_empty() {
                    None
                } else {
                    serde_json::from_str(s).ok()
                }
            }),
            size,
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

    // Check if a new game can be created (no existing game or game is finished)
    pub fn can_create_new(&self) -> bool {
        matches!(
            self.game_state,
            StacksSweeperGameState::Won | StacksSweeperGameState::Lost
        )
    }

    // Update claim state based on game outcome
    pub fn update_claim_state_on_game_end(&mut self) {
        match self.game_state {
            StacksSweeperGameState::Won => {
                // If won and haven't claimed yet, set to NotClaimed
                if self.claim_state.is_none() {
                    self.claim_state = Some(ClaimState::NotClaimed);
                }
            }
            StacksSweeperGameState::Lost => {
                // If lost, set claim state to None (no claimable reward)
                self.claim_state = None;
            }
            _ => {
                // For Waiting/Playing states, keep current claim state
            }
        }
    }

    // Check if player can cashout (game won or in progress with reveals)
    pub fn can_cashout(&self) -> bool {
        match self.game_state {
            StacksSweeperGameState::Won => {
                // Can cashout if won and not claimed yet
                matches!(self.claim_state, Some(ClaimState::NotClaimed))
            }
            StacksSweeperGameState::Playing => {
                // Can cashout during game if has revealed cells and not claimed
                self.user_revealed_count > 0
                    && matches!(self.claim_state, Some(ClaimState::NotClaimed))
            }
            _ => false,
        }
    }

    // Get cashout amount (amount * current multiplier)
    pub fn get_cashout_amount(&self) -> Option<f64> {
        if self.can_cashout() {
            let current_multiplier = calc_cashout_multiplier(
                self.size,
                self.risk as f64,
                self.user_revealed_count as usize,
            );
            Some(self.amount * current_multiplier)
        } else {
            None
        }
    }

    // Get the total number of mines in the game
    pub fn get_mine_count(&self) -> u32 {
        self.cells.iter().filter(|cell| cell.is_mine).count() as u32
    }

    // Get masked cells (only showing revealed information)
    pub fn get_masked_cells(&self) -> Vec<MaskedCell> {
        self.cells
            .iter()
            .map(|cell| {
                let state = if cell.flagged {
                    Some(CellState::Flagged)
                } else if cell.revealed {
                    if cell.is_mine {
                        Some(CellState::Mine)
                    } else if self.blind {
                        Some(CellState::Gem)
                    } else {
                        Some(CellState::Adjacent {
                            count: cell.adjacent,
                        })
                    }
                } else {
                    None
                };

                MaskedCell {
                    x: cell.x,
                    y: cell.y,
                    state,
                }
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

fn risk_scale(d: f64, hard_density: f64, gamma: f64) -> f64 {
    (d / hard_density).powf(gamma)
}

/// Preview function: full clear multiplier for chosen board size + difficulty.
pub fn _calc_target_multiplier(n: usize, d: f64) -> f64 {
    let base = 2.0; // Hard 5x5 full clear anchor
    let beta = 0.1; // board size growth factor
    let hard_density = 0.4;
    let gamma = 0.8;

    let size_scale = 1.0 + beta * ((n as f64) - 5.0);
    let risk = risk_scale(d, hard_density, gamma);

    let target = base * size_scale * risk;
    (target * 100.0).floor() / 100.0 // round down to 2 decimals
}

/// Cashout function: multiplier based on revealed safe cells so far.
pub fn calc_cashout_multiplier(n: usize, d: f64, r: usize) -> f64 {
    let base = 2.0;
    let beta = 0.1;
    let hard_density = 0.4;
    let gamma = 0.8;

    let t = (n * n) as f64;
    let m = (t * d).round();
    let s = t - m;
    let c = (s - 1.0).max(1.0); // avoid divide by zero

    let size_scale = 1.0 + beta * ((n as f64) - 5.0);
    let risk = risk_scale(d, hard_density, gamma);

    let target = base * size_scale * risk;
    let p = (target - 1.0) / c;

    let multiplier = 1.0 + p * (r as f64);
    (multiplier * 100.0).floor() / 100.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CellState {
    Hidden,
    Flagged,
    Mine,
    Gem,
    Adjacent { count: u8 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaskedCell {
    pub x: usize,
    pub y: usize,
    pub state: Option<CellState>, // None if not revealed and not flagged
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EliminationReason {
    HitMine,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStanding {
    pub player: crate::models::game::Player,
    pub rank: usize,
}
