mod config;
mod discord;
mod inter_comm;
mod twitch;

use std::env::var;
use std::fs;

use crate::config::Config;
use crate::inter_comm::InterComm;
use tokio::join;
use tokio::sync::mpsc;
use tracing::debug;
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

    let config = serde_json::from_str::<Config>(
        &fs::read_to_string(var("CONFIG_PATH").unwrap_or(String::from("./config.json")))
            .expect("Error while reading config file"),
    )
    .expect("Error while parsing config file");

    let (tx, rx) = mpsc::channel::<InterComm>(32);

    let (_, _) = join!(
        discord::bot::run(tx.clone(), rx, &config),
        twitch::websocket::run(tx, &config.twitch_watcher)
    );
}
