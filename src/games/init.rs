use crate::{
    db::game::{
        delete::clear_all_games, get::get_all_games, post::create_game, words::add_word_set,
    },
    errors::AppError,
    state::RedisClient,
};

pub async fn initialize_games(redis: RedisClient) -> Result<(), AppError> {
    tracing::info!("Initializing games...");

    // Initialize word set
    add_word_set(redis.clone()).await?;

    // Try to get all games from Redis
    match get_all_games(redis.clone()).await {
        Ok(games) => {
            if games.len() < 2 {
                tracing::info!(
                    "Found {} games, need to reinitialize with all games",
                    games.len()
                );

                // Clear existing games
                clear_all_games(redis.clone()).await?;

                // Add all games
                add_games(redis).await?;
            } else {
                tracing::info!("Found {} existing games in database", games.len());
            }
        }
        Err(e) => {
            tracing::warn!("Failed to get games from database: {}", e);
        }
    }

    Ok(())
}

async fn add_games(redis: RedisClient) -> Result<(), AppError> {
    // Add Lexi Wars
    let lexi_wars_id = create_game(
        "Lexi Wars".to_string(),
        "A word battle game where players compete with words.".to_string(),
        "https://res.cloudinary.com/dapbvli1v/image/upload/Lexi_Wars2_yuuoam.png".to_string(),
        Some(vec![
            "word".to_string(),
            "strategy".to_string(),
            "multiplayer".to_string(),
        ]),
        2,
        redis.clone(),
    )
    .await?;

    tracing::info!("Added Lexi Wars game with ID: {}", lexi_wars_id);

    // Add Stacks Sweepers
    let stacks_sweepers_id = create_game(
        "Stacks Sweepers".to_string(),
        "A minesweeper game where players sweep tiles, avoid traps, and compete for rewards."
            .to_string(),
        "https://res.cloudinary.com/dapbvli1v/image/upload/Lexi_Wars2_yuuoam.png".to_string(),
        Some(vec![
            "puzzle".to_string(),
            "strategy".to_string(),
            "singleplayer".to_string(),
            "multiplayer".to_string(),
        ]),
        1,
        redis,
    )
    .await?;

    tracing::info!("Added Stacks Sweepers game with ID: {}", stacks_sweepers_id);

    Ok(())
}
