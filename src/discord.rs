use crate::message::Message;
use poise::serenity_prelude as serenity;
use serenity::all::ChannelId;
use serenity::builder::EditChannel;
use std::ops::Deref;
use tokio::sync::mpsc::Receiver;
use tracing::info;

// Types used by all command functions
type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, (), Error>;

pub async fn run(token: String, receiver: Receiver<Message>) -> Result<(), Error> {
    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::MESSAGE_CONTENT;

    let framework = poise::Framework::builder()
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(())
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

    let mut client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await?;

    client.start().await?;

    Ok(())
}

async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, (), Error>,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Ready { data_about_bot, .. } => {
            info!("Logged in as {}", data_about_bot.user.name);
        }
        // serenity::FullEvent::Message { new_message } => {}
        serenity::FullEvent::VoiceStateUpdate { old, new } => {
            info!("VoiceStateUpdate {:?} -> {:?}", old, new)
        }
        _ => {}
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
