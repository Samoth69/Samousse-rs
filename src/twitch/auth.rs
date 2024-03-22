use std::env::var;
use std::fmt::Error;
use std::fs;
use std::fs::File;
use std::io::Write;

use anyhow::anyhow;
use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

#[derive(Serialize, Deserialize, Debug)]
pub struct TwitchToken {
    pub access_token: String,
    pub refresh_token: String,
}

pub fn get_client_ids() -> (String, String) {
    (
        var("TWITCH_CLIENT_ID").expect("Missing TWITCH_CLIENT_ID"),
        var("TWITCH_CLIENT_SECRET").expect("Missing TWITCH_CLIENT_SECRET"),
    )
}

impl TwitchToken {
    pub async fn new() -> anyhow::Result<TwitchToken> {
        let cred = get_client_ids();
        let config_path = var("TWITCH_CACHE_PATH").unwrap_or(String::from("./twitch_cache.json"));

        let http_client = reqwest::Client::new();

        let mut twitch_token =
            serde_json::from_str::<TwitchToken>(&fs::read_to_string(&config_path).unwrap_or(
                String::from("{\"access_token\":\"\",\"refresh_token\":\"\"}"),
            ))?;

        if twitch_token.access_token.is_empty() {
            if let Ok(env) = var("TWITCH_ACCESS_TOKEN") {
                twitch_token.access_token = env;
                info!("Filled access token from env vars");
            }
        }
        if twitch_token.refresh_token.is_empty() {
            if let Ok(env) = var("TWITCH_REFRESH_TOKEN") {
                twitch_token.refresh_token = env;
                info!("Filled refresh token from env vars");
            }
        }

        debug!("Loaded TwitchToken file, checking validity");
        let res = http_client
            .get("https://id.twitch.tv/oauth2/validate")
            .header(
                header::AUTHORIZATION,
                "Bearer ".to_owned() + &twitch_token.access_token,
            )
            .send()
            .await?;
        if res.status() == StatusCode::OK {
            debug!("Token is valid");
        } else {
            info!("Token expired, trying to logging");
            let res = http_client
                .post("https://id.twitch.tv/oauth2/token")
                .form(&vec![
                    ("client_id", cred.0),
                    ("client_secret", cred.1),
                    ("grant_type", String::from("refresh_token")),
                    ("refresh_token", twitch_token.refresh_token),
                ])
                .send()
                .await?;

            if res.status() != StatusCode::OK {
                panic!("Auth failed");
            }

            twitch_token = res.json::<TwitchToken>().await?;
        }

        let mut fi = File::create(&config_path)?;
        fi.write_all(serde_json::to_string(&twitch_token)?.as_bytes())?;

        Ok(twitch_token)
    }
}
