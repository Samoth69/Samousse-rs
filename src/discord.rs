use std::collections::HashMap;
use std::sync::Arc;

use poise::serenity_prelude as serenity;
use serenity::all::{ChannelId, GuildId, UserId};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, RwLock};

use crate::inter_comm::InterComm;

pub mod bot;
mod message_response;
mod random_stuff;
mod twitch;

// Types used by all command functions
type Error = Box<dyn std::error::Error + Send + Sync>;
type DiscordContext<'a> = poise::Context<'a, Data, Error>;

#[derive(Debug)]
struct Data {
    pub trusted_users_ids: Arc<Vec<u64>>,
    pub twitch: Arc<RwLock<DiscordTwitchWatcher>>,
    pub sender: Mutex<Sender<InterComm>>,
    pub receiver: Mutex<Option<Receiver<InterComm>>>,
    pub activity_messages: Vec<String>,
    pub question_answers: Arc<Vec<String>>,
    pub random_answers: Arc<Vec<String>>,
}

#[derive(Debug)]
struct DiscordTwitchWatcher {
    // contain channels that have been altered by the bot
    pub channels: HashMap<ChannelId, Channel>,
    // list of twitch channel with tied discord account to monitor
    pub users: HashMap<UserId, User>,
    pub renamed_channel_name: String,
    pub enabled: bool,
    // contains servers where DiscordTwitchWatch should operate (excluding server not in this list)
    pub servers: Vec<GuildId>,
}

#[derive(Debug)]
struct Channel {
    pub original_name: String,
}

#[derive(Debug)]
struct User {
    pub discord_id: UserId,
    // may contain the current channel for the specified user
    pub current_channel_id: Option<ChannelId>,
    // will became true if a voice state event has mentionned this user
    // false otherwise
    pub has_been_part_of_voice_state_event: bool,
    pub twitch_id: u64,
    pub twitch_is_streaming: Option<bool>,
}

impl DiscordTwitchWatcher {
    pub fn find_user_by_twitch_id(&self, twitch_id: u64) -> Option<&User> {
        self.users
            .iter()
            .find(|f| f.1.twitch_id == twitch_id)
            .map(|m| m.1)
    }
    pub fn find_user_by_twitch_id_mut(&mut self, twitch_id: u64) -> Option<&mut User> {
        self.users
            .iter_mut()
            .find(|f| f.1.twitch_id == twitch_id)
            .map(|m| m.1)
    }
    pub fn find_user_in_channel(&self, channel_id: ChannelId) -> Vec<&User> {
        self.users
            .iter()
            .filter(|f| match f.1.current_channel_id {
                Some(channel) => channel == channel_id,
                None => false,
            })
            .map(|m| m.1)
            .collect()
    }
}
