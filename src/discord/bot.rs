use std::collections::HashMap;
use std::env::var;
use std::sync::Arc;

use poise::builtins::on_error;
use poise::serenity_prelude as serenity;
use rand::seq::SliceRandom;
use serenity::all::{ActivityData, GuildId, UserId};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, trace};

use crate::config::Config;
use crate::discord::message_response::handle_message;
use crate::discord::random_stuff::{echo, ping, random_number};
use crate::discord::twitch::{
    rename_channel, status, twitch_event_handler, update_streaming_status,
};
use crate::discord::{Data, DiscordTwitchWatcher, Error, User};
use crate::inter_comm::InterComm;

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
            commands: vec![
                ping(),
                echo(),
                random_number(),
                status(),
                update_streaming_status(),
            ],
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
                    question_answers: Arc::new(config.question_answers),
                    random_answers: Arc::new(config.random_answers),
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

            if framework.user_data.twitch.read().await.enabled {
                let receiver = framework.user_data.receiver.lock().await.take();
                let twitch = framework.user_data.twitch.clone();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    twitch_event_handler(&ctx, &mut receiver.unwrap(), twitch)
                        .await
                        .unwrap();
                });
            }
        }
        serenity::FullEvent::Message { new_message } => {
            if new_message.author.bot {
                trace!("Skipping message sent by bot {}", new_message.author.name);
            } else {
                trace!("Received message {:?}", new_message);
                if let Err(why) = handle_message(
                    ctx,
                    framework.user_data.question_answers.clone(),
                    framework.user_data.random_answers.clone(),
                    new_message,
                )
                .await
                {
                    error!("Error on handling message {}", why);
                }
            }
        }
        serenity::FullEvent::VoiceStateUpdate { old, new } => {
            trace!("VoiceStateUpdate {:?} -> {:?}", old, new);
            let mut is_known_user = false;
            {
                let mut data = framework.user_data.twitch.write().await;
                match data.users.get_mut(&new.user_id) {
                    Some(user) => {
                        is_known_user = true;
                        user.has_been_part_of_voice_state_event = true;
                        user.current_channel_id = new.channel_id;
                    }
                    None => trace!("User {} isn't monitored", new.user_id),
                }
            }
            if is_known_user {
                let twitch = framework.user_data.twitch.clone();
                let is_streaming: Option<bool> = twitch
                    .read()
                    .await
                    .users
                    .get(&new.user_id)
                    .map_or_else(|| None, |v| v.twitch_is_streaming);
                if let Some(is_streaming) = is_streaming {
                    if is_streaming {
                        if let Some(old) = old {
                            if let Some(channel_id) = old.channel_id {
                                // We want to rename the old channel back
                                rename_channel(
                                    ctx,
                                    twitch.clone(),
                                    &old.user_id,
                                    &channel_id,
                                    false,
                                )
                                .await?;
                            }
                        }
                        if let Some(channel_id) = new.channel_id {
                            rename_channel(
                                ctx,
                                twitch.clone(),
                                &new.user_id,
                                &channel_id,
                                is_streaming,
                            )
                            .await?;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}
