with recursive post_thread_messages as (
    select chat_id, message_id
    from telegram_messages
    where source_channel_id is not null

    union

    select child.chat_id, child.message_id
    from telegram_messages child
    join post_thread_messages parent
      on parent.chat_id = child.chat_id
     and parent.message_id = child.reply_to_message_id
    where child.source_channel_id is null
), post_comment_counts as materialized (
    select messages.chat_id,
           messages.user_id as telegram_user_id,
           count(*)::bigint as reply_count
    from post_thread_messages thread
    join telegram_messages messages
      on messages.chat_id = thread.chat_id
     and messages.message_id = thread.message_id
    where messages.user_id is not null
      and messages.source_channel_id is null
    group by messages.chat_id, messages.user_id
), user_counts as (
    select users.chat_id,
           users.telegram_user_id,
           coalesce(counts.reply_count, 0)::bigint as reply_count
    from telegram_chat_users users
    left join post_comment_counts counts
      on counts.chat_id = users.chat_id
     and counts.telegram_user_id = users.telegram_user_id
)
update telegram_chat_users users
set reply_to_channel_post_count = user_counts.reply_count,
    updated_at = now()
from user_counts
where users.chat_id = user_counts.chat_id
  and users.telegram_user_id = user_counts.telegram_user_id
  and users.reply_to_channel_post_count is distinct from user_counts.reply_count;
