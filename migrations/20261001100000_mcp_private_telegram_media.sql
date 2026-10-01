-- Internal lookup for the bounded chat.get_media tool. This view is deliberately
-- outside mcp_public and is not listed in the public database manifest.
create schema if not exists mcp_private;
revoke all on schema mcp_private from public;

create or replace view mcp_private.telegram_media as
select m.chat_id,
       m.message_id,
       'photo'::text as media_kind,
       photo.value ->> 'file_id' as file_id,
       case
           when (photo.value ->> 'file_size') ~ '^[0-9]{1,18}$' then
               case
                   when (photo.value ->> 'file_size')::numeric < 4294967295
                       then (photo.value ->> 'file_size')::bigint
                   else null
               end
           else null
       end as file_size,
       null::text as file_name
from public.telegram_messages m
cross join lateral (
    select item.value
    from jsonb_array_elements(
        case when jsonb_typeof(m.raw_json -> 'photo') = 'array'
             then m.raw_json -> 'photo' else '[]'::jsonb end
    ) with ordinality as item(value, ordinality)
    order by item.ordinality desc
    limit 1
) as photo
where m.chat_id = -1001932061163
  and m.has_photo
  and m.deleted_by_bot_at is null
  and m.spam_marked_at is null
  and nullif(btrim(photo.value ->> 'file_id'), '') is not null

union all

select m.chat_id,
       m.message_id,
       'document'::text as media_kind,
       m.raw_json #>> '{document,file_id}' as file_id,
       case
           when (m.raw_json #>> '{document,file_size}') ~ '^[0-9]{1,18}$' then
               case
                   when (m.raw_json #>> '{document,file_size}')::numeric < 4294967295
                       then (m.raw_json #>> '{document,file_size}')::bigint
                   else null
               end
           else null
       end as file_size,
       nullif(m.raw_json #>> '{document,file_name}', '') as file_name
from public.telegram_messages m
where m.chat_id = -1001932061163
  and m.has_document
  and m.deleted_by_bot_at is null
  and m.spam_marked_at is null
  and nullif(btrim(m.raw_json #>> '{document,file_id}'), '') is not null;

revoke all on mcp_private.telegram_media from public;

do $$
begin
    if exists (select 1 from pg_roles where rolname = 'nedobot_mcp_ro') then
        execute 'grant usage on schema mcp_private to nedobot_mcp_ro';
        execute 'grant select on mcp_private.telegram_media to nedobot_mcp_ro';
    end if;
end $$;
