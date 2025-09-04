use rand::seq::SliceRandom;
use rand::rng;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Cell {
    pub x: usize,
    pub y: usize,
    pub is_mine: bool,
    pub adjacent: u8,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Board {
    pub size: usize,
    pub cells: Vec<Cell>, // flattened grid
}

impl Board {
    pub fn generate(size: usize, risk: f32) -> Self {
        assert!(risk >= 0.0 && risk <= 1.0, "risk must be between 0.0 and 1.0");

        let total_cells = size * size;
        let mine_count = ((total_cells as f32) * risk).round() as usize;

        // Step 1: choose random mine positions
        let mut positions: Vec<usize> = (0..total_cells).collect();
        positions.shuffle(&mut rng());
        let mine_positions: Vec<usize> = positions.into_iter().take(mine_count).collect();

        // Step 2: build cells
        let mut cells: Vec<Cell> = Vec::with_capacity(total_cells);

        for y in 0..size {
            for x in 0..size {
                let idx = y * size + x;
                let is_mine = mine_positions.contains(&idx);

                // Count adjacent mines
                let mut adjacent = 0;
                if !is_mine {
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            if dx == 0 && dy == 0 {
                                continue;
                            }
                            let nx = x as isize + dx;
                            let ny = y as isize + dy;
                            if nx >= 0 && nx < size as isize && ny >= 0 && ny < size as isize {
                                let n_idx = (ny as usize) * size + (nx as usize);
                                if mine_positions.contains(&n_idx) {
                                    adjacent += 1;
                                }
                            }
                        }
                    }
                }

                cells.push(Cell { x, y, is_mine, adjacent });
            }
        }

        Self { size, cells }
    }
}
