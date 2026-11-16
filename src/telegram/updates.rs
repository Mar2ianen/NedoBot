use teloxide::{
    prelude::*,
    types::{ChatMemberUpdated, MessageReactionCountUpdated, MessageReactionUpdated},
};

use crate::{
    db::telegram::{save_chat_member_event, save_message_reaction, save_message_reaction_count},
    features::{
        ingest::{is_managed_chat, managed_chat_allows},
        member_greetings::handle_chat_member_update,
    },
    state::AppState,
};

pub async fn handle_message_reaction(
    reaction: MessageReactionUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if !managed_chat_allows(&state.config, reaction.chat.id.0, |chat| chat.ingest) {
        return Ok(());
    }
    if let Err(err) = save_message_reaction(&state.pool, &reaction).await {
        tracing::error!(%err, "failed to save message reaction");
    }

    Ok(())
}

pub async fn handle_message_reaction_count(
    reaction_count: MessageReactionCountUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if !managed_chat_allows(&state.config, reaction_count.chat.id.0, |chat| chat.ingest) {
        return Ok(());
    }
    if let Err(err) = save_message_reaction_count(&state.pool, &reaction_count).await {
        tracing::error!(%err, "failed to save message reaction count");
    }

    Ok(())
}

pub async fn handle_chat_member(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    member: ChatMemberUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if !is_managed_chat(&state.config, member.chat.id.0) {
        return Ok(());
    }
    if managed_chat_allows(&state.config, member.chat.id.0, |chat| chat.ingest)
        && let Err(err) = save_chat_member_event(&state.pool, &member).await
    {
        tracing::error!(%err, "failed to save chat member event");
    }
    if let Err(err) = handle_chat_member_update(&bot, &member, &state.config).await {
        tracing::warn!(%err, chat_id = member.chat.id.0, "failed to send configured welcome or farewell message");
    }

    Ok(())
}
