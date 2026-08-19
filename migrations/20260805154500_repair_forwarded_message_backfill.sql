update public.telegram_messages
set is_forwarded = (
    is_automatic_forward
    or raw_json -> 'forward_origin' is not null
    or nullif(raw_json ->> 'forwarded_from', '') is not null
    or nullif(raw_json ->> 'forwarded_from_id', '') is not null
)
where is_forwarded <> (
    is_automatic_forward
    or raw_json -> 'forward_origin' is not null
    or nullif(raw_json ->> 'forwarded_from', '') is not null
    or nullif(raw_json ->> 'forwarded_from_id', '') is not null
);
