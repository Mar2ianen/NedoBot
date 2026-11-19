alter table telegram_user_profiles
    add column if not exists profile_refresh_failure_count integer not null default 0,
    add column if not exists profile_refresh_retry_at timestamptz;

-- Existing rows with a recorded failure get one bounded cooldown instead of
-- retrying immediately on the next message after the deploy.
update telegram_user_profiles
set profile_refresh_failure_count = 1,
    profile_refresh_retry_at = now() + interval '1 minute'
where profile_refresh_error is not null
  and profile_refresh_retry_at is null;
