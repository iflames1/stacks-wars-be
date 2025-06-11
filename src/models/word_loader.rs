use std::{collections::HashSet, sync::Arc};

use once_cell::sync::Lazy;

pub fn load_word_list() -> HashSet<String> {
    let json = include_str!("../assets/words.json");
    serde_json::from_str(json).expect("Failed to parse words.json")
}

pub static WORD_LIST: Lazy<Arc<HashSet<String>>> = Lazy::new(|| Arc::new(load_word_list()));
