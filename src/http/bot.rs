use html_escape::encode_text;
use reqwest::Url;
use teloxide::{
    Bot,
    payloads::SendPhotoSetters,
    prelude::{Request, Requester},
    types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, ParseMode},
};

use crate::models::game::GameType;
use uuid::Uuid;

pub struct BotNewLobbyPayload {
    pub lobby_id: Uuid,
    pub lobby_name: String,
    pub description: Option<String>,
    pub game: GameType,
    pub contract_address: Option<String>,
    pub entry_amount: Option<f64>,
    pub creator_name: Option<String>,
    pub wallet_address: String,
}

pub async fn broadcast_lobby_created(
    bot: &Bot,
    chat_id: i64,
    payload: BotNewLobbyPayload,
) -> Result<(), teloxide::RequestError> {
    let wallet = payload.wallet_address.clone();
    let truncated_wallet = format!("{}...{}", &wallet[0..4], &wallet[wallet.len() - 4..]);

    let lobby_name = format!(
        "🏷 <b>Lobby Name:</b> {}\n",
        encode_text(&payload.lobby_name)
    );

    let game_name = format!("🎮 <b>Game:</b> {}\n", encode_text(&payload.game.name));

    let creator = payload
        .creator_name
        .as_ref()
        .map(|name| {
            format!(
                "🧑‍🚀 <b>Creator:</b> {} ({})\n",
                encode_text(name),
                encode_text(&truncated_wallet)
            )
        })
        .unwrap_or_else(|| format!("🧑‍🚀 <b>Creator:</b> {}\n", encode_text(&wallet)));

    let description = payload
        .description
        .as_ref()
        .map(|desc| format!("📝 <b>Description:</b> {}\n", encode_text(desc)))
        .unwrap_or_default();

    let contract_line = payload
        .contract_address
        .as_ref()
        .map(|addr| {
            format!(
                "💰 <b>Pool Contract:</b> <a href=\"https://explorer.hiro.so/txid/{}?chain=testnet\">View on Hiro</a>\n",
                addr
            )
        })
        .unwrap_or_default();

    let entry_fee_line = payload
        .entry_amount
        .map(|amount| format!("💵 <b>Entry Fee:</b> {} STX\n", amount))
        .unwrap_or_default();

    let lobby_link = format!(
        "\n🔗 <b>Link:</b> <code>https://stackswars.com/lobby/{}</code>",
        payload.lobby_id
    );

    let caption = format!(
        "🆕 <b>New Lobby Created</b>\n\n\
        {lobby_name}\
        {game_name}\
        {creator}\
        {description}\
        {contract_line}\
        {entry_fee_line}\
        {lobby_link}",
    );

    tracing::info!("Telegram caption (HTML): {}", caption);

    let lobby_url: Url = Url::parse(&format!(
        "https://stackswars.com/lobby/{}",
        payload.lobby_id
    ))
    .unwrap();

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
        "🚀 Join Now",
        lobby_url,
    )]]);

    bot.send_photo(
        ChatId(chat_id),
        InputFile::url(payload.game.image_url.parse().unwrap()),
    )
    .caption(caption)
    .parse_mode(ParseMode::Html) // ✅ Switched to HTML
    .reply_markup(keyboard)
    .send()
    .await?;

    Ok(())
}
