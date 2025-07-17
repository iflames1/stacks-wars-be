use reqwest::Url;
use teloxide::{
    Bot,
    payloads::SendPhotoSetters,
    prelude::{Request, Requester},
    types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, ParseMode},
};

use uuid::Uuid;

pub struct BotNewLobbyPayload {
    pub room_id: Uuid,
    pub room_name: String,
    pub description: Option<String>,
    pub game_name: String,
    pub game_image: String,
    pub contract_address: Option<String>,
    pub creator_display_name: Option<String>,
    pub wallet_address: String,
}

pub async fn broadcast_lobby_created(
    bot: &Bot,
    chat_id: i64,
    payload: BotNewLobbyPayload,
) -> Result<(), teloxide::RequestError> {
    let contract_line = payload
        .contract_address
        .as_ref()
        .map(|addr| {
            format!(
                "💰 *Pool Contract:* [View on Hiro](https://explorer.hiro.so/txid/{}\n)",
                addr
            )
        })
        .unwrap_or_default();

    let description = payload
        .description
        .as_ref()
        .map(|desc| format!("📝 *Description:* {}\n", desc))
        .unwrap_or_default();

    let caption = format!(
        "🆕 *New Lobby Created*\n\n\
        🏷 *Name:* {}\n\
        🎮 *Game:* {}\n\
        🧑‍🚀 *Creator:* {}\n\
        {}{}",
        payload.room_name,
        payload.game_name,
        payload
            .creator_display_name
            .unwrap_or(payload.wallet_address.clone()),
        description,
        contract_line,
    );

    let lobby_link: Url =
        Url::parse(&format!("https://stackswars.com/lobby/{}", payload.room_id)).unwrap();
    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
        "🚀 Join Now",
        lobby_link,
    )]]);

    bot.send_photo(
        ChatId(chat_id),
        InputFile::url(payload.game_image.parse().unwrap()),
    )
    .caption(caption)
    .parse_mode(ParseMode::MarkdownV2)
    .reply_markup(keyboard)
    .send()
    .await?;

    Ok(())
}
