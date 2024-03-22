use crate::discord::{DiscordContext, Error};
use rand::{Rng, thread_rng};
use tracing::debug;

// this function is used in poise::command attributes to check if the user is trustworthy
// copy-paste from https://github.com/serenity-rs/poise/blob/current/examples/feature_showcase/checks.rs#L47
pub async fn is_trusted(ctx: DiscordContext<'_>) -> Result<bool, Error> {
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
pub async fn ping(ctx: DiscordContext<'_>) -> Result<(), Error> {
    ctx.say("pong !").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn echo(ctx: DiscordContext<'_>, message: String) -> Result<(), Error> {
    ctx.say(message).await?;
    Ok(())
}
#[poise::command(
    slash_command,
    description_localized(
        "en",
        "Generate a random number between provided numbers (min and max included)"
    ),
    description_localized(
        "fr",
        "Génère un nombre aléatoire entre le minimum et le maximum fourni, min et max inclus"
    )
)]
pub async fn random_number(
    ctx: DiscordContext<'_>,
    #[description = "Included"]
    #[description_localized("fr", "Inclus")]
    min: i32,
    #[description = "Included"]
    #[description_localized("fr", "Inclus")]
    max: i32,
    #[description = "Hide result"]
    #[description_localized("fr", "Cacher le résultat")]
    spoiler: bool,
) -> Result<(), Error> {
    if min > max {
        ctx.say("Min must be higher than max").await?;
        return Ok(());
    }
    let x: i32 = thread_rng().gen_range(min..=max);
    let result = if spoiler {
        format!("De {} à {} : ||{}||", min, max, x)
    } else {
        format!("De {} à {} : {}", min, max, x)
    };
    ctx.say(result).await?;
    Ok(())
}
