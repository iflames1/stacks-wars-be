use crate::errors::AppError;

pub async fn validate_payment_tx(
    tx_id: &str,
    expected_sender: &str,
    expected_contract: &str,
    expected_amount: u64,
) -> Result<(), AppError> {
    let url = format!("https://api.testnet.hiro.so/extended/v1/tx/{}", tx_id);

    let res = reqwest::get(&url)
        .await
        .map_err(|e| AppError::BadRequest(format!("Failed to fetch tx: {}", e)))?;

    if !res.status().is_success() {
        return Err(AppError::BadRequest(format!(
            "Transaction not found or failed: {}",
            tx_id
        )));
    }

    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| AppError::Deserialization(format!("Invalid JSON response: {}", e)))?;

    // Validate sender
    let sender_address = json
        .get("sender_address")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("Missing sender address".into()))?;

    if sender_address != expected_sender {
        return Err(AppError::BadRequest(format!(
            "Unexpected sender address: {}",
            sender_address
        )));
    }

    // Validate transaction status
    let status = json
        .get("tx_status")
        .and_then(|v| v.as_str())
        .unwrap_or("failed");

    if status != "success" {
        return Err(AppError::BadRequest("Transaction failed".into()));
    }

    // Validate amount and recipient
    let empty_vec = Vec::new();
    let events = json
        .get("events")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty_vec);
    let transfer = events.iter().find(|e| {
        e.get("event_type").and_then(|et| et.as_str()) == Some("stx_asset")
            && e.get("asset").and_then(|asset| {
                let recipient =
                    asset.get("recipient").and_then(|r| r.as_str()) == Some(expected_contract);
                let amount = asset
                    .get("amount")
                    .and_then(|a| a.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                    == Some(expected_amount);
                Some(recipient && amount)
            }) == Some(true)
    });

    if transfer.is_none() {
        return Err(AppError::BadRequest(
            "Payment transfer event not found or doesn't match".into(),
        ));
    }

    Ok(())
}
