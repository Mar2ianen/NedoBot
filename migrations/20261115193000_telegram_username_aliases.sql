-- Current alias index derived from authoritative Telegram profile/chat snapshots.
-- Historical profile names remain in telegram_profile_identity_observations.
-- This table only accelerates current username -> numeric peer resolution.
create table if not exists public.telegram_username_aliases (
    username_normalized text not null,
    telegram_peer_id bigint not null,
    peer_type text not null
        check (peer_type in ('user', 'bot', 'private_chat', 'group', 'supergroup', 'channel')),
    source text not null
        check (source in ('user_update', 'profile_refresh', 'configured_chat_refresh', 'known_bot_refresh')),
    observed_at timestamptz not null default now(),
    is_active boolean not null default true,
    primary key (username_normalized, telegram_peer_id, peer_type, source)
);

create index if not exists telegram_username_aliases_lookup_idx
    on public.telegram_username_aliases (username_normalized, observed_at desc)
    where is_active;

create index if not exists telegram_username_aliases_peer_idx
    on public.telegram_username_aliases (telegram_peer_id, peer_type, observed_at desc);

