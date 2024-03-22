use anyhow::anyhow;
use std::sync::Arc;

use poise::serenity_prelude as serenity;
use rand::seq::SliceRandom;
use serenity::all::{Message, UserId};
use serenity::builder::CreateMessage;

pub async fn handle_message(
    ctx: &serenity::Context,
    question_answers: Arc<Vec<String>>,
    random_answers: Arc<Vec<String>>,
    message: &Message,
) -> anyhow::Result<()> {
    if is_samousse_mentioned(ctx.cache.current_user().id, message) {
        let is_question: bool = message.content.contains('?');
        let msg = CreateMessage::new().content(select_random_entry(match is_question {
            true => question_answers,
            false => random_answers,
        })?);
        message.channel_id.send_message(&ctx.http, msg).await?;
    }
    Ok(())
}

fn select_random_entry(arr: Arc<Vec<String>>) -> anyhow::Result<String> {
    if arr.is_empty() {
        return Err(anyhow!("Array is empty"));
    }
    Ok(arr.choose(&mut rand::thread_rng()).unwrap().clone())
}

fn is_samousse_mentioned(bot_user_id: UserId, msg: &Message) -> bool {
    msg.mentions.iter().any(|m| m.id == bot_user_id)
        || msg.content.to_lowercase().contains("samousse")
}
