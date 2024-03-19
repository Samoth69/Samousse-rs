use crate::config::Config;
use crate::inter_comm::{InterComm, MessageType};
use poise::serenity_prelude as serenity;
use std::env::var;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info};

// Types used by all command functions
type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;

struct Data {
    pub twitch: Arc<RwLock<DiscordTwitchWatcher>>,
    pub receiver: Mutex<Option<Receiver<InterComm>>>,
}

struct DiscordTwitchWatcher {
    // contain channels that have been altered by the bot
    pub channels: Vec<Channel>,
    // list of twitch channel with tied discord account to monitor
    pub users: Vec<User>,
    pub renamed_channel_name: String,
    pub renamed_channel_topic: String,
    pub enabled: bool,
}

struct Channel {
    pub channel_id: i64,
    pub original_name: String,
}

struct User {
    pub discord_id: i64,
    // None if unknown
    pub discord_username: Option<String>,

    pub twitch_id: i64,
    pub twitch_is_streaming: Option<bool>,
}

pub async fn run(mut receiver: Receiver<InterComm>, config: &Config) -> Result<(), Error> {
    let discord_token = var("DISCORD_TOKEN").expect("Missing DISCORD_TOKEN");

    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::MESSAGE_CONTENT;

    let config = config.to_owned();
    let framework = poise::Framework::builder()
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(Data {
                    twitch: Arc::new(RwLock::new(DiscordTwitchWatcher {
                        channels: vec![],
                        users: config
                            .twitch_watcher
                            .channels
                            .iter()
                            .map(|m| User {
                                twitch_id: m.twitch_channel_id,
                                discord_id: m.discord_id,
                                discord_username: None,
                                twitch_is_streaming: None,
                            })
                            .collect(),
                        renamed_channel_name: config.twitch_watcher.renamed_channel_name,
                        renamed_channel_topic: config.twitch_watcher.renamed_channel_topic,
                        enabled: config.twitch_watcher.enabled,
                    })),
                    receiver: Mutex::new(Some(receiver)),
                })
            })
        })
        .options(poise::FrameworkOptions {
            event_handler: |ctx, event, framework, _| {
                Box::pin(event_handler(ctx, event, framework))
            },
            commands: vec![ping()],
            ..Default::default()
        })
        .build();

    let mut client = serenity::ClientBuilder::new(discord_token, intents)
        .framework(framework)
        .await?;

    client.start().await?;

    Ok(())
}

async fn event_handler(
    _ctx: &serenity::Context,
    event: &serenity::FullEvent,
    framework: poise::FrameworkContext<'_, Data, Error>,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Ready { data_about_bot, .. } => {
            info!("Logged in as {}", data_about_bot.user.name);
            let receiver = framework.user_data.receiver.lock().await.take();
            let twitch = framework.user_data.twitch.clone();
            tokio::spawn(async move {
                twitch_event_handler(&mut receiver.unwrap(), twitch)
                    .await
                    .unwrap();
            });
        }
        serenity::FullEvent::Message { new_message } => {
            debug!("Received message {:?}", new_message);
        }
        serenity::FullEvent::VoiceStateUpdate { old, new } => {
            info!("VoiceStateUpdate {:?} -> {:?}", old, new)
        }
        _ => {}
    }
    Ok(())
}

async fn twitch_event_handler(
    receiver: &mut Receiver<InterComm>,
    twitch: Arc<RwLock<DiscordTwitchWatcher>>,
) -> anyhow::Result<()> {
    while let Some(item) = receiver.recv().await {
        match item.message_type {
            MessageType::TwitchStreamOnline => {
                debug!("Handling twitch stream online event");
            }
            MessageType::TwitchStreamOffline => {
                debug!("Handling twitch stream offline event");
            }
        }
    }
    Ok(())
}

#[poise::command(slash_command)]
async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say("Pong !").await?;

    // let id = ChannelId::new(883418664777437227);
    //
    // let builder = EditChannel::new()
    //     .name("EN LIVE")
    //     .topic("✞ Eniram ✞ est en stream");
    //
    // ctx.http()
    //     .edit_channel(id, &builder, Some("Twitch event"))
    //     .await?;

    Ok(())
}
