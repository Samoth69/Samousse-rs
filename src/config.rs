use std::num::NonZeroU64;
use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
pub struct TwitchUser {
    pub twitch_channel_id: NonZeroU64,
    pub discord_id: NonZeroU64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct TwitchWatcher {
    pub servers: Vec<u64>,
    pub channels: Vec<TwitchUser>,
    pub renamed_channel_name: String,
    pub enabled: bool,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    pub activity_messages: Vec<String>,
    pub question_answers: Vec<String>,
    pub random_answers: Vec<String>,
    pub trusted_users: Vec<u64>,
    pub twitch_watcher: TwitchWatcher,
}
