use std::sync::Arc;

use anyhow::anyhow;
use poise::serenity_prelude as serenity;
use serenity::all::{ChannelId, EditChannel, UserId};
use tokio::sync::mpsc::Receiver;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

use crate::discord::{
    random_stuff::is_trusted, Channel, DiscordContext, DiscordTwitchWatcher, Error,
};
use crate::inter_comm::{InterComm, MessageType};

pub async fn twitch_event_handler(
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

pub async fn handle_stream_event(
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
            discord_user_id = Some(u.discord_id);
            if u.twitch_is_streaming != Some(is_streaming) {
                u.twitch_is_streaming = Some(is_streaming);
            } else {
                debug!(
                    "Discord user {} twitch streaming status hasn't changed",
                    u.discord_id
                );
                return Ok(());
            }
        }
        None => warn!("Unknown user {:?} from twitch side", streamer_user_id),
    }
    if let Some(discord_user_id) = discord_user_id {
        if let Some(channel_id) =
            find_current_user_voice_channel(ctx, twitch.clone(), &discord_user_id).await?
        {
            rename_channel(ctx, twitch, &discord_user_id, &channel_id, is_streaming).await?;
        } else {
            debug!("Discord user {} not found in channel", discord_user_id);
        }
    } else {
        return Err(anyhow!("Unknown twitch user id {}", streamer_user_id));
    }
    Ok(())
}

pub async fn find_current_user_voice_channel(
    ctx: &serenity::Context,
    twitch: Arc<RwLock<DiscordTwitchWatcher>>,
    discord_user_id: &UserId,
) -> anyhow::Result<Option<ChannelId>> {
    let mut ret: Option<ChannelId> = None;
    if let Some(user) = twitch.read().await.users.get(discord_user_id) {
        if user.has_been_part_of_voice_state_event {
            ret = user.current_channel_id;
        } else {
            debug!("searching user current voice channel slow way");
            for server in twitch.read().await.servers.iter() {
                if let Some(guild) = ctx.cache.guild(*server) {
                    if let Some(streamer) = guild.voice_states.get(&user.discord_id) {
                        ret = streamer.channel_id;
                    }
                }
            }
        }
    }

    if let Some(channel_id) = ret {
        if let Some(user) = twitch.write().await.users.get_mut(discord_user_id) {
            user.has_been_part_of_voice_state_event = true;
            user.current_channel_id = Some(channel_id);
        } else {
            warn!("Discord user {} doesn't exist", discord_user_id);
        }
    } else {
        debug!(
            "Discord user {} not found in voice channel",
            discord_user_id
        );
    }

    Ok(ret)
}

pub async fn get_channel_new_name<'a>(
    ctx: &'a serenity::Context,
    twitch: Arc<RwLock<DiscordTwitchWatcher>>,
    discord_user_id: &'a UserId,
    channel_id: &ChannelId,
    is_streaming: bool,
) -> anyhow::Result<Option<(ChannelId, EditChannel<'a>, Option<String>)>> {
    // will be true if the channel has already been renamed
    let channel_has_been_renamed = twitch.read().await.channels.contains_key(channel_id);

    if twitch
        .read()
        .await
        .find_user_in_channel(*channel_id)
        .iter()
        .filter(|f| f.twitch_is_streaming.is_some_and(|a| a) && channel_has_been_renamed)
        .count()
        >= 1
    {
        debug!("We do not change channel name, more than one streamer are in this channel");
        return Ok(None);
    } else if is_streaming {
        if channel_has_been_renamed {
            info!("Channel {} is already renamed", channel_id);
            return Ok(None);
        } else {
            debug!("Channel {} need to be renamed", channel_id);
        }
    }

    // actual name of the channel on Discord
    let discord_channel_name: String;
    if let Some(discord_channel) = ctx.cache.channel(channel_id) {
        discord_channel_name = discord_channel.name.clone();
    } else {
        return Err(anyhow!("Channel {} doesn't exist", channel_id));
    }

    // the new name of the channel
    let new_channel_name: String;
    {
        trace!("before write lock");
        let mut writer = twitch.write().await;
        trace!("after write lock");
        new_channel_name = if is_streaming {
            let to_insert = Channel {
                original_name: discord_channel_name,
            };
            writer.channels.insert(*channel_id, to_insert);
            writer.renamed_channel_name.clone()
        } else {
            let to_restore = writer.channels.remove(channel_id).unwrap();
            to_restore.original_name
        };
    }

    let reason = match is_streaming {
        true => format!("User {} is streaming", discord_user_id),
        false => format!("User {} has stopped his stream", discord_user_id),
    };
    let edit_channel = EditChannel::new().name(new_channel_name);

    Ok(Some((*channel_id, edit_channel, Some(reason))))
}

pub async fn rename_channel(
    ctx: &serenity::Context,
    twitch: Arc<RwLock<DiscordTwitchWatcher>>,
    discord_user_id: &UserId,
    channel_id: &ChannelId,
    is_streaming: bool,
) -> anyhow::Result<()> {
    debug!("Renaming channel");
    match get_channel_new_name(ctx, twitch, discord_user_id, channel_id, is_streaming).await? {
        Some((a, b, c)) => {
            debug!("Editing channel {:?} {:?} {:?}", a, b, c);
            if let Err(why) = ctx.http.edit_channel(a, &b, c.as_deref()).await {
                error!("Error on channel rename {}", why);
            } else {
                debug!("Done editing channel");
            }
        }
        None => {
            debug!("None returned from get_channel_new_name");
        }
    }

    Ok(())
}

#[poise::command(slash_command, check = "is_trusted")]
pub async fn status(ctx: DiscordContext<'_>) -> Result<(), Error> {
    let text: String;
    {
        let reader = ctx.data().twitch.read().await;
        let mut channels = reader
            .channels
            .iter()
            .map(|m| format!("{}:{}", m.0, m.1.original_name))
            .collect::<Vec<String>>()
            .join(", ");
        if channels.is_empty() {
            channels = String::from("None");
        }
        text = format!(
            "Monitoring {} streams\n\
        Online stream count : {}\n\
        Renamed channels : {}",
            reader.users.len(),
            reader
                .users
                .iter()
                .filter(|f| f.1.twitch_is_streaming == Some(true))
                .count(),
            channels
        );
    }
    ctx.say(text).await?;

    Ok(())
}

#[poise::command(slash_command, check = "is_trusted")]
pub async fn update_streaming_status(
    ctx: DiscordContext<'_>,
    user: serenity::User,
    is_streaming: bool,
) -> Result<(), Error> {
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
            message_type: match is_streaming {
                true => MessageType::TwitchStreamOnline,
                false => MessageType::TwitchStreamOffline,
            },
            streamer_user_id: twitch_user_id,
            streamer_user_login: twitch_user_login,
        })
        .await?;

    ctx.say("ok").await?;
    Ok(())
}
