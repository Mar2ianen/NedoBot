with duplicate_reactions as (
    select id,
           row_number() over (
               partition by chat_id,
                            message_id,
                            coalesce(user_id, 0),
                            coalesce(actor_chat_id, 0),
                            event_at,
                            new_reactions
               order by id
           ) as rn
    from telegram_message_reactions
)
delete from telegram_message_reactions
where id in (
    select id
    from duplicate_reactions
    where rn > 1
);

drop index if exists telegram_message_reactions_unique_event_idx;

create unique index telegram_message_reactions_unique_event_idx
    on telegram_message_reactions (
        chat_id,
        message_id,
        coalesce(user_id, 0),
        coalesce(actor_chat_id, 0),
        event_at,
        new_reactions
    );
