use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformRating {
    pub id: Uuid,
    pub user_id: Uuid,
    pub rating: i16,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
}
