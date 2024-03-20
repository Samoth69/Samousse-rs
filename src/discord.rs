use crate::config::Config;
use crate::inter_comm::{InterComm, MessageType};
use anyhow::anyhow;
use poise::builtins::on_error;
use poise::serenity_prelude as serenity;
use rand::seq::SliceRandom;
use serenity::all::{Activity, ActivityData, ChannelId, GuildId, UserId};
use serenity::builder::EditChannel;
use std::collections::HashMap;
use std::env::var;
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, trace, warn};

// Types used by all command functions
type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;

#[derive(Debug)]
struct Data {
    pub trusted_users_ids: Arc<Vec<u64>>,
    pub twitch: Arc<RwLock<DiscordTwitchWatcher>>,
    pub sender: Mutex<Sender<InterComm>>,
    pub receiver: Mutex<Option<Receiver<InterComm>>>,
    pub activity_messages: Vec<String>,
}

#[derive(Debug)]
struct DiscordTwitchWatcher {
    // contain channels that have been altered by the bot
    pub channels: HashMap<ChannelId, Channel>,
    // list of twitch channel with tied discord account to monitor
    pub users: HashMap<UserId, User>,
    pub renamed_channel_name: String,
    pub renamed_channel_topic: String,
    pub enabled: bool,
    // contains servers where DiscordTwitchWatch should operate (excluding server not in this list)
    pub servers: Vec<GuildId>,
}

#[derive(Debug)]
struct Channel {
    pub original_name: String,
    pub original_topic: Option<String>,
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
}

pub async fn run(
    sender: Sender<InterComm>,
    receiver: Receiver<InterComm>,
    config: &Config,
) -> Result<(), Error> {
    let discord_token = var("DISCORD_TOKEN").expect("Missing DISCORD_TOKEN");

    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::MESSAGE_CONTENT;

    let config = config.to_owned();
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            event_handler: |ctx, event, framework, _| {
                Box::pin(event_handler(ctx, event, framework))
            },
            commands: vec![ping(), echo(), status(), mark_as_streaming()],
            on_error: |error| {
                Box::pin(async move {
                    if let Err(e) = on_error(error).await {
                        error!("Fatal error while sending error message: {}", e);
                    }
                })
            },
            ..Default::default()
        })
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                let mut users: HashMap<UserId, User> = HashMap::new();
                for m in config.twitch_watcher.channels {
                    users.insert(
                        UserId::from(m.discord_id),
                        User {
                            twitch_id: m.twitch_channel_id.into(),
                            discord_id: UserId::from(m.discord_id),
                            current_channel_id: None,
                            twitch_is_streaming: None,
                            has_been_part_of_voice_state_event: false,
                        },
                    );
                }
                Ok(Data {
                    trusted_users_ids: Arc::new(config.trusted_users),
                    twitch: Arc::new(RwLock::new(DiscordTwitchWatcher {
                        channels: HashMap::new(),
                        users,
                        renamed_channel_name: config.twitch_watcher.renamed_channel_name,
                        renamed_channel_topic: config.twitch_watcher.renamed_channel_topic,
                        enabled: config.twitch_watcher.enabled,
                        servers: config
                            .twitch_watcher
                            .servers
                            .iter()
                            .map(|v| GuildId::from(*v))
                            .collect(),
                    })),
                    receiver: Mutex::new(Some(receiver)),
                    sender: Mutex::new(sender),
                    activity_messages: config.activity_messages,
                })
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(discord_token, intents)
        .framework(framework)
        .await?;

    client.start().await?;

    Ok(())
}

async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    framework: poise::FrameworkContext<'_, Data, Error>,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Ready { data_about_bot, .. } => {
            info!("Logged in as {}", data_about_bot.user.name);

            ctx.set_activity(
                framework
                    .user_data
                    .activity_messages
                    .choose(&mut rand::thread_rng())
                    .map(|m| Some(ActivityData::custom(m)))
                    .unwrap_or(None),
            );

            let receiver = framework.user_data.receiver.lock().await.take();
            let twitch = framework.user_data.twitch.clone();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                twitch_event_handler(&ctx, &mut receiver.unwrap(), twitch)
                    .await
                    .unwrap();
            });
        }
        serenity::FullEvent::Message { new_message } => {
            if new_message.author.bot {
                trace!("Skipping message sent by bot {}", new_message.author.name);
            } else {
                debug!("Received message {:?}", new_message);
            }
        }
        serenity::FullEvent::VoiceStateUpdate { old, new } => {
            info!("VoiceStateUpdate {:?} -> {:?}", old, new);
            let mut data = framework.user_data.twitch.write().await;
            match data.users.get_mut(&new.user_id) {
                Some(user) => {
                    user.has_been_part_of_voice_state_event = true;
                    user.current_channel_id = new.channel_id;
                }
                None => trace!("User {} isn't monitored", new.user_id),
            }
        }
        _ => {}
    }
    Ok(())
}

async fn twitch_event_handler(
    ctx: &serenity::Context,
    receiver: &mut Receiver<InterComm>,
    twitch: Arc<RwLock<DiscordTwitchWatcher>>,
) -> anyhow::Result<()> {
    while let Some(item) = receiver.recv().await {
        match item.message_type {
            MessageType::TwitchStreamOnline => {
                debug!("Handling twitch stream online event");
                if let Err(why) = handle_stream_event(
                    ctx,
                    twitch.clone(),
                    item.streamer_user_id.parse().unwrap(),
                    true,
                )
                .await
                {
                    error!("Error on twitch stream online event handling {}", why);
                }
            }
            MessageType::TwitchStreamOffline => {
                debug!("Handling twitch stream offline event");
                if let Err(why) = handle_stream_event(
                    ctx,
                    twitch.clone(),
                    item.streamer_user_id.parse().unwrap(),
                    false,
                )
                .await
                {
                    error!("Error on twitch stream online event handling {}", why);
                }
            }
        }
    }
    Ok(())
}

async fn handle_stream_event(
    ctx: &serenity::Context,
    twitch: Arc<RwLock<DiscordTwitchWatcher>>,
    streamer_user_id: u64,
    is_streaming: bool,
) -> anyhow::Result<()> {
    let mut discord_user_id: Option<UserId> = None;
    match twitch
        .write()
        .await
        .find_user_by_twitch_id_mut(streamer_user_id)
    {
        Some(u) => {
            u.twitch_is_streaming = Some(is_streaming);
            discord_user_id = Some(u.discord_id);
        }
        None => warn!("Unknown user {:?} from twitch side", streamer_user_id),
    }
    if let Some(discord_user_id) = discord_user_id {
        rename_channel(ctx, twitch, &discord_user_id, is_streaming).await?;
    } else {
        return Err(anyhow!("Unknown twitch user id {}", streamer_user_id));
    }
    Ok(())
}

async fn find_current_user_voice_channel(
    ctx: &serenity::Context,
    twitch: Arc<RwLock<DiscordTwitchWatcher>>,
    discord_user_id: &UserId,
) -> anyhow::Result<Option<ChannelId>> {
    if let Some(user) = twitch.read().await.users.get(discord_user_id) {
        if user.has_been_part_of_voice_state_event {
            return Ok(user.current_channel_id);
        } else {
            debug!("searching user current voice channel slow way");
            trace!("Before read lock");
            let data = twitch.read().await;
            trace!("Read lock acquired");
            for server in data.servers.iter() {
                if let Some(guild) = ctx.cache.guild(*server) {
                    if let Some(streamer) = guild.voice_states.get(&user.discord_id) {
                        return Ok(streamer.channel_id);
                    }
                }
            }
        }
    }

    debug!(
        "Discord user {} not found in voice channel",
        discord_user_id
    );
    Ok(None)
}

async fn get_channel_new_name<'a>(
    ctx: &'a serenity::Context,
    twitch: Arc<RwLock<DiscordTwitchWatcher>>,
    discord_user_id: &'a UserId,
    is_streaming: bool,
) -> anyhow::Result<Option<(ChannelId, EditChannel<'a>, Option<String>)>> {
    let name: String;
    let topic: Option<String>;
    if let Some(channel_id) =
        find_current_user_voice_channel(ctx, twitch.clone(), discord_user_id).await?
    {
        trace!("Channel id {} found in data", channel_id);
        if let Some(channel) = twitch.read().await.channels.get(&channel_id) {
            info!(
                "Channel {} is already marked has streaming",
                channel.original_name
            );
            return Ok(None);
        }
        trace!("before write lock");
        let mut writer = twitch.write().await;
        trace!("write lock acquired");
        if let Some(discord_channel) = ctx.cache.channel(channel_id) {
            trace!("Discord channel exist in ctx cache");

            if is_streaming {
                let to_insert = Channel {
                    original_name: discord_channel.name.clone(),
                    original_topic: discord_channel.topic.clone(),
                };
                writer.channels.insert(channel_id, to_insert);
                name = writer.renamed_channel_name.clone();
                topic = Some(writer.renamed_channel_topic.replace("{username}", "abcd"));
            } else {
                let to_restore = writer.channels.remove(&channel_id).unwrap();
                name = to_restore.original_name;
                topic = to_restore.original_topic;
            }

            let reason = match is_streaming {
                true => format!("User {} is streaming", discord_user_id),
                false => format!("User {} has stopped his stream", discord_user_id),
            };
            let mut edit_channel = EditChannel::new().name(name);
            if let Some(topic) = topic {
                edit_channel = edit_channel.topic(topic);
            }

            return Ok(Some((channel_id, edit_channel, Some(reason))));
        }
        return Err(anyhow!("Channel {} doesn't exist", channel_id));
    } else {
        warn!("Discord user {} not found in channel", discord_user_id);
    }
    Ok(None)
}

async fn rename_channel(
    ctx: &serenity::Context,
    twitch: Arc<RwLock<DiscordTwitchWatcher>>,
    discord_user_id: &UserId,
    is_streaming: bool,
) -> anyhow::Result<()> {
    debug!("Renaming channel");
    match get_channel_new_name(ctx, twitch, discord_user_id, is_streaming).await? {
        Some((a, b, c)) => {
            debug!("Editting channel {:?} {:?} {:?}", a, b, c);
            if let Err(why) = ctx.http.edit_channel(a, &b, c.as_deref()).await {
                error!("Error on channel rename {}", why);
            }
        }
        None => {
            debug!("None returned from get_channel_new_name");
        }
    }

    Ok(())
}

// this function is used in poise::command attributes to check if the user is trustworthy
// copy-paste from https://github.com/serenity-rs/poise/blob/current/examples/feature_showcase/checks.rs#L47
async fn is_trusted(ctx: Context<'_>) -> Result<bool, Error> {
    let ret = ctx
        .data()
        .trusted_users_ids
        .contains(&ctx.author().id.get());
    if !ret {
        ctx.say("You aren't trusted enough to do this").await?;
        debug!(
            "User {} (aka {}) isn't in trusted_users",
            ctx.author().id,
            ctx.author().name
        );
    }
    Ok(ret)
}

#[poise::command(slash_command)]
async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say("pong !").await?;
    Ok(())
}

#[poise::command(slash_command)]
async fn echo(ctx: Context<'_>, message: String) -> Result<(), Error> {
    ctx.say(message).await?;
    Ok(())
}

#[poise::command(slash_command, check = "is_trusted")]
async fn status(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say(format!("{:?}", ctx.data().twitch.read().await))
        .await?;

    Ok(())
}

#[poise::command(slash_command, check = "is_trusted")]
async fn mark_as_streaming(ctx: Context<'_>, user: serenity::User) -> Result<(), Error> {
    let twitch_user_id: String;
    let twitch_user_login: String;
    if let Some(local_user) = ctx.data().twitch.read().await.users.get(&user.id) {
        twitch_user_id = local_user.twitch_id.to_string();
        twitch_user_login = user.name.to_lowercase();
    } else {
        ctx.say("User isn't registered").await?;
        return Ok(());
    }

    ctx.data()
        .sender
        .lock()
        .await
        .send(InterComm {
            message_type: MessageType::TwitchStreamOnline,
            streamer_user_id: twitch_user_id,
            streamer_user_login: twitch_user_login,
        })
        .await?;

    ctx.say("ok").await?;
    Ok(())
}
