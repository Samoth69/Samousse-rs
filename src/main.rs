mod config;
mod discord;
mod message;
mod twitch;

use std::env::var;
use std::fs;

use crate::config::Config;
use crate::message::Message;
use tokio::join;
use tokio::sync::mpsc;
use tracing::{debug, error};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(fmt::layer())
        .init();
    debug!("We are in debug mode");

    let discord_token = var("DISCORD_TOKEN").expect("Missing DISCORD_TOKEN");
    let twitch_token = var("TWITCH_TOKEN").expect("Missing TWITCH_TOKEN");

    let config = json5::from_str::<Config>(
        &fs::read_to_string(var("CONFIG_PATH").unwrap_or(String::from("./config.json5")))
            .expect("Error while reading config file"),
    )
    .expect("Error while parsing config file");

    let (tx, rx) = mpsc::channel::<Message>(32);
    
    join!(
        discord::run(discord_token, rx),
        twitch::websocket::run(twitch_token, tx, config.twitch_watcher)
    );
}
