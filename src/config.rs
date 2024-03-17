use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct TwitchUser {
    pub twitch_channel_id: i64,
    pub discord_id: i64,
}

#[derive(Deserialize, Debug)]
pub struct TwitchWatcher {
    pub channels: Vec<TwitchUser>,
    pub renamed_channel_name: String,
    pub renamed_channel_topic: String,
    pub enabled: bool,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub twitch_watcher: TwitchWatcher,
}
