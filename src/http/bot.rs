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
    pub entry_amount: Option<f64>,
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
                "💰 *Pool Contract:* [View on Hiro](https://explorer.hiro.so/txid/{}?chain=testnet)\n",
                addr
            )
        })
        .unwrap_or_default();

    let entry_fee_line = payload
        .entry_amount
        .map(|amount| format!("💵 *Entry Fee:* {} STX\n", amount))
        .unwrap_or_default();

    let description = payload
        .description
        .as_ref()
        .map(|desc| format!("📝 *Description:* {}\n", desc))
        .unwrap_or_default();

    let lobby_link = format!("https://stackswars.com/lobby/{}", payload.room_id);
    let lobby_url: Url = Url::parse(&lobby_link).unwrap();

    let caption = format!(
        "🆕 *New Lobby Created*\n\n\
        🏷 *Name:* {}\n\
        🎮 *Game:* {}\n\
        🧑‍🚀 *Creator:* {}\n\
        {}{}{}\
        \n🔗 *Link:* `{}`",
        payload.room_name,
        payload.game_name,
        payload
            .creator_display_name
            .unwrap_or(payload.wallet_address.clone()),
        description,
        contract_line,
        entry_fee_line,
        lobby_link
    );

    // Create keyboard with join button only
    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
        "🚀 Join Now",
        lobby_url,
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
