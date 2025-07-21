use uuid::Uuid;

use crate::{
    db::game::{get::get_all_games, post::add_game},
    errors::AppError,
    models::game::GameType,
    state::RedisClient,
};

pub async fn initialize_games(redis: RedisClient) -> Result<(), AppError> {
    tracing::info!("Initializing games...");

    // Try to get all games from Redis
    match get_all_games(redis.clone()).await {
        Ok(games) => {
            if games.is_empty() {
                tracing::info!("No games found in database, adding default games");
                add_default_games(redis).await?;
            } else {
                tracing::info!("Found {} existing games in database", games.len());
            }
        }
        Err(e) => {
            tracing::warn!(
                "Failed to get games from database: {}, adding default games",
                e
            );
            add_default_games(redis).await?;
        }
    }

    Ok(())
}

async fn add_default_games(redis: RedisClient) -> Result<(), AppError> {
    let lexi_wars_game = GameType {
        id: Uuid::new_v4(),
        name: "Lexi Wars".to_string(),
        description: "A word battle game where players compete with words.".to_string(),
        image_url: "https://res.cloudinary.com/dapbvli1v/image/upload/Lexi_Wars2_yuuoam.png"
            .to_string(),
        tags: vec![
            "word".to_string(),
            "strategy".to_string(),
            "multiplayer".to_string(),
        ]
        .into(),
        min_players: 2,
    };

    let game_id = add_game(lexi_wars_game.clone(), redis).await?;
    tracing::info!("Added Lexi Wars game with ID: {}", game_id);

    Ok(())
}
