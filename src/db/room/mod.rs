pub mod get;
pub mod post;
pub mod put;

pub use get::{get_all_lobby_info, get_lobby_info};
pub use post::create_lobby;
pub use put::{
    join_room, leave_room, update_claim_state, update_game_state, update_player_state,
    update_room_player_after_game,
};
