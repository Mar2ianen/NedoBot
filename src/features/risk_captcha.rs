use serde_json::Value;
use sqlx::{PgPool, Row, postgres::PgRow};
use teloxide::{
    payloads::{
        EditMessageTextSetters, RestrictChatMemberSetters, SendMessageSetters,
        UnbanChatMemberSetters,
    },
    prelude::{Bot, Requester},
    types::{
        ChatId, ChatMemberKind, ChatPermissions, InlineKeyboardButton, InlineKeyboardMarkup, UserId,
    },
};
use uuid::Uuid;

use crate::features::new_user_audit::repo::NewUserAuditJob;

const MAX_WRONG_ANSWERS: i32 = 3;
const MAX_SETUP_ATTEMPTS: i32 = 5;
const SETUP_LEASE_SECONDS: i64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptchaOutcome {
    Issued,
    AlreadyPending,
    AlreadyPassed,
    FailedAttempts,
    SetupFailed,
    Overridden,
    Expired,
    Ineligible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptchaCallbackOutcome {
    Passed,
    PassedButOtherRestrictionRemains,
    Wrong { attempts_left: i32 },
    Failed,
    Expired,
    Stale,
    NotForThisUser,
}

impl CaptchaCallbackOutcome {
    pub fn text(self) -> &'static str {
        match self {
            Self::Passed => "Проверка пройдена, ограничения сняты.",
            Self::PassedButOtherRestrictionRemains => {
                "Проверка пройдена. Другое ограничение остаётся, обратитесь к модератору."
            }
            Self::Wrong { .. } => "Неверный ответ.",
            Self::Failed => "Попытки закончились. Ограничение остаётся; обратитесь к модератору.",
            Self::Expired => "Время на проверку истекло.",
            Self::Stale => "Эта проверка уже недействительна.",
            Self::NotForThisUser => "Эта проверка предназначена другому участнику.",
        }
    }

    pub fn attempts_left(self) -> Option<i32> {
        match self {
            Self::Wrong { attempts_left } => Some(attempts_left),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Passed
                | Self::PassedButOtherRestrictionRemains
                | Self::Failed
                | Self::Expired
                | Self::Stale
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MathChallenge {
    id: Uuid,
    question: String,
    options: Vec<String>,
    correct_option: i16,
}

#[derive(Debug, Clone)]
struct CaptchaChallenge {
    id: Uuid,
    chat_id: i64,
    user_id: i64,
    question: String,
    options: Vec<String>,
    correct_option: i16,
    restore_permissions: ChatPermissions,
    restriction_applied_at: Option<chrono::DateTime<chrono::Utc>>,
    message_id: Option<i32>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    status: String,
    setup_attempts: i32,
}

/// Создаёт durable challenge и запускает его доставку. Незавершённая настройка
/// также подхватывается отдельным bounded worker после рестарта процесса.
pub async fn apply_risk_captcha(
    bot: &Bot,
    pool: &PgPool,
    job: &NewUserAuditJob,
    risk_score: i32,
    ttl_seconds: i64,
) -> anyhow::Result<CaptchaOutcome> {
    let challenge = match load_user_challenge(pool, job.chat_id, job.telegram_user_id).await? {
        Some(challenge) if challenge.status == "expired" => {
            let member = bot
                .get_chat_member(ChatId(job.chat_id), UserId(job.telegram_user_id as u64))
                .await?;
            if !matches!(member.kind, ChatMemberKind::Member(_)) {
                return Ok(CaptchaOutcome::Expired);
            }
            let chat = bot.get_chat(ChatId(job.chat_id)).await?;
            let restore_permissions = chat.permissions().unwrap_or_else(ChatPermissions::all);
            let math = generate_math_challenge(Uuid::new_v4());
            let permissions_json = serde_json::to_value(restore_permissions)?;
            reopen_expired_challenge(pool, job, risk_score, &math, &permissions_json).await?
        }
        Some(challenge) => challenge,
        None => {
            let member = bot
                .get_chat_member(ChatId(job.chat_id), UserId(job.telegram_user_id as u64))
                .await?;
            if !matches!(member.kind, ChatMemberKind::Member(_)) {
                return Ok(CaptchaOutcome::Ineligible);
            }
            let chat = bot.get_chat(ChatId(job.chat_id)).await?;
            let restore_permissions = chat.permissions().unwrap_or_else(ChatPermissions::all);
            let math = generate_math_challenge(Uuid::new_v4());
            let permissions_json = serde_json::to_value(restore_permissions)?;
            create_or_load_challenge(pool, job, risk_score, &math, &permissions_json).await?
        }
    };

    match challenge.status.as_str() {
        "passed" => return Ok(CaptchaOutcome::AlreadyPassed),
        "failed" => return Ok(CaptchaOutcome::FailedAttempts),
        "setup_failed" => return Ok(CaptchaOutcome::SetupFailed),
        "overridden" => return Ok(CaptchaOutcome::Overridden),
        "expired" => return Ok(CaptchaOutcome::Expired),
        "solving" => return Ok(CaptchaOutcome::AlreadyPending),
        "pending" if challenge.message_id.is_some() => {
            return Ok(CaptchaOutcome::AlreadyPending);
        }
        "preparing" | "setting_up" | "pending" => {}
        _ => return Ok(CaptchaOutcome::Overridden),
    }
    let Some(claimed) = claim_setup_by_id(pool, challenge.id).await? else {
        return Ok(CaptchaOutcome::AlreadyPending);
    };
    run_claimed_setup(bot, pool, &claimed, ttl_seconds).await
}

/// Берёт одну доставку капчи из PostgreSQL с `SKIP LOCKED`. Lease возвращает
/// зависшую настройку в очередь после падения процесса; число попыток ограничено.
pub async fn process_next_setup(
    bot: &Bot,
    pool: &PgPool,
    ttl_seconds: i64,
) -> anyhow::Result<bool> {
    let Some(challenge) = claim_next_setup(pool).await? else {
        return Ok(false);
    };
    run_claimed_setup(bot, pool, &challenge, ttl_seconds).await?;
    Ok(true)
}

/// Завершает одну истёкшую капчу. Claim lease позволяет повторить kick после
/// рестарта, если Telegram принял только часть ban/unban последовательности.
pub async fn process_next_expired(bot: &Bot, pool: &PgPool) -> anyhow::Result<bool> {
    let Some(challenge) = claim_next_expired(pool).await? else {
        return Ok(false);
    };
    expire_claimed_challenge(bot, pool, &challenge).await?;
    Ok(true)
}

async fn run_claimed_setup(
    bot: &Bot,
    pool: &PgPool,
    challenge: &CaptchaChallenge,
    ttl_seconds: i64,
) -> anyhow::Result<CaptchaOutcome> {
    match deliver_challenge(bot, pool, challenge, ttl_seconds).await {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            retry_or_finish_setup(bot, pool, challenge).await?;
            Err(error)
        }
    }
}

async fn deliver_challenge(
    bot: &Bot,
    pool: &PgPool,
    challenge: &CaptchaChallenge,
    ttl_seconds: i64,
) -> anyhow::Result<CaptchaOutcome> {
    if !ensure_restricted(bot, pool, challenge).await? {
        return Ok(CaptchaOutcome::Overridden);
    }

    crate::features::auto_moderation::delete_first_message_for_user_required(
        bot,
        pool,
        challenge.chat_id,
        challenge.user_id,
    )
    .await?;
    let text = format!(
        "Проверка нового участника\nРешите пример: {}.\nУ вас {} мин. До успешного ответа отправка сообщений ограничена; после истечения времени бот удалит вас из чата.",
        challenge.question,
        ttl_seconds.saturating_add(59) / 60
    );
    let sent = bot
        .send_message(ChatId(challenge.chat_id), text)
        .reply_markup(keyboard(challenge))
        .await?;
    set_message_sent(pool, challenge.id, sent.id.0, ttl_seconds).await?;
    Ok(CaptchaOutcome::Issued)
}

async fn retry_or_finish_setup(
    bot: &Bot,
    pool: &PgPool,
    challenge: &CaptchaChallenge,
) -> anyhow::Result<()> {
    if challenge.setup_attempts >= MAX_SETUP_ATTEMPTS {
        let restored = restore_after_failed_delivery(bot, pool, challenge).await?;
        let status = if restored {
            "setup_failed"
        } else {
            "overridden"
        };
        set_status(pool, challenge.id, status).await?;
        tracing::error!(
            chat_id = challenge.chat_id,
            user_id = challenge.user_id,
            attempts = challenge.setup_attempts,
            status,
            "risk captcha setup exhausted its retry budget"
        );
        return Ok(());
    }

    let delay_seconds = 2_i64
        .pow(challenge.setup_attempts.clamp(1, 6) as u32)
        .min(60);
    sqlx::query(
        "update telegram_risk_captcha_challenges set status = 'preparing', setup_next_attempt_at = now() + ($2 * interval '1 second'), setup_lease_expires_at = null, setup_error_kind = 'setup_attempt_failed', updated_at = now() where id = $1 and status = 'setting_up'",
    )
    .bind(challenge.id)
    .bind(delay_seconds)
    .execute(pool)
    .await?;
    Ok(())
}

pub fn is_callback_data(data: &str) -> bool {
    data.starts_with("rc:")
}

/// Проверяет владельца inline-кнопки и снимает только созданное капчей
/// ограничение. Перед восстановлением Telegram-права и журнал ручной
/// модерации сверяются, чтобы не отменить параллельный mute администратора.
pub async fn handle_callback(
    bot: &Bot,
    pool: &PgPool,
    callback_chat_id: Option<i64>,
    actor_user_id: i64,
    data: &str,
) -> anyhow::Result<CaptchaCallbackOutcome> {
    let Some((challenge_id, selected_option)) = parse_callback_data(data) else {
        return Ok(CaptchaCallbackOutcome::Stale);
    };
    let Some(challenge) = load_challenge(pool, challenge_id).await? else {
        return Ok(CaptchaCallbackOutcome::Stale);
    };
    if callback_chat_id != Some(challenge.chat_id) {
        return Ok(CaptchaCallbackOutcome::Stale);
    }
    if actor_user_id != challenge.user_id {
        return Ok(CaptchaCallbackOutcome::NotForThisUser);
    }
    if challenge
        .expires_at
        .is_some_and(|expires_at| expires_at <= chrono::Utc::now())
    {
        return Ok(CaptchaCallbackOutcome::Expired);
    }
    if !matches!(
        challenge.status.as_str(),
        "preparing" | "setting_up" | "pending" | "solving"
    ) {
        return Ok(match challenge.status.as_str() {
            "failed" => CaptchaCallbackOutcome::Failed,
            "expired" | "expiring" => CaptchaCallbackOutcome::Expired,
            _ => CaptchaCallbackOutcome::Stale,
        });
    }
    if selected_option != challenge.correct_option {
        return record_wrong_answer(pool, challenge.id).await;
    }
    if !mark_solving(pool, challenge.id).await? {
        return Ok(CaptchaCallbackOutcome::Stale);
    }

    #[cfg(feature = "manual-moderation")]
    if crate::features::manual_moderation::active_restriction_action(
        pool,
        challenge.chat_id,
        challenge.user_id,
    )
    .await?
    .is_some()
    {
        set_status(pool, challenge.id, "overridden").await?;
        return Ok(CaptchaCallbackOutcome::PassedButOtherRestrictionRemains);
    }

    let member = bot
        .get_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
        .await?;
    match member.kind {
        ChatMemberKind::Member(_) => {
            set_passed(pool, challenge.id).await?;
            Ok(CaptchaCallbackOutcome::Passed)
        }
        ChatMemberKind::Restricted(restricted)
            if restricted.is_member && has_no_permissions(&restricted) =>
        {
            if challenge.restriction_applied_at.is_none() {
                set_status(pool, challenge.id, "overridden").await?;
                return Ok(CaptchaCallbackOutcome::PassedButOtherRestrictionRemains);
            }
            restore_permissions(bot, &challenge).await?;
            set_passed(pool, challenge.id).await?;
            Ok(CaptchaCallbackOutcome::Passed)
        }
        _ => {
            set_status(pool, challenge.id, "overridden").await?;
            Ok(CaptchaCallbackOutcome::PassedButOtherRestrictionRemains)
        }
    }
}

async fn ensure_restricted(
    bot: &Bot,
    pool: &PgPool,
    challenge: &CaptchaChallenge,
) -> anyhow::Result<bool> {
    let member = bot
        .get_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
        .await?;
    match member.kind {
        ChatMemberKind::Member(_) => {
            if challenge.restriction_applied_at.is_some() {
                set_status(pool, challenge.id, "overridden").await?;
                return Ok(false);
            }
            bot.restrict_chat_member(
                ChatId(challenge.chat_id),
                UserId(challenge.user_id as u64),
                ChatPermissions::empty(),
            )
            .use_independent_chat_permissions(true)
            .await?;
            mark_restriction_applied(pool, challenge.id).await?;
            Ok(true)
        }
        ChatMemberKind::Restricted(restricted)
            if restricted.is_member && has_no_permissions(&restricted) =>
        {
            if challenge.restriction_applied_at.is_none() {
                set_status(pool, challenge.id, "overridden").await?;
                return Ok(false);
            }
            Ok(true)
        }
        _ => {
            set_status(pool, challenge.id, "overridden").await?;
            Ok(false)
        }
    }
}

async fn restore_permissions(bot: &Bot, challenge: &CaptchaChallenge) -> anyhow::Result<()> {
    bot.restrict_chat_member(
        ChatId(challenge.chat_id),
        UserId(challenge.user_id as u64),
        challenge.restore_permissions.clone(),
    )
    .use_independent_chat_permissions(true)
    .await?;
    Ok(())
}

async fn restore_after_failed_delivery(
    bot: &Bot,
    pool: &PgPool,
    challenge: &CaptchaChallenge,
) -> anyhow::Result<bool> {
    #[cfg(feature = "manual-moderation")]
    if crate::features::manual_moderation::active_restriction_action(
        pool,
        challenge.chat_id,
        challenge.user_id,
    )
    .await?
    .is_some()
    {
        return Ok(false);
    }

    let member = bot
        .get_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
        .await?;
    match member.kind {
        ChatMemberKind::Member(_) => Ok(true),
        ChatMemberKind::Restricted(restricted)
            if restricted.is_member && has_no_permissions(&restricted) =>
        {
            if challenge.restriction_applied_at.is_none() {
                return Ok(false);
            }
            restore_permissions(bot, challenge).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn has_no_permissions(member: &teloxide::types::Restricted) -> bool {
    !member.can_send_messages
        && !member.can_send_audios
        && !member.can_send_documents
        && !member.can_send_photos
        && !member.can_send_videos
        && !member.can_send_video_notes
        && !member.can_send_voice_notes
        && !member.can_send_other_messages
        && !member.can_add_web_page_previews
        && !member.can_send_polls
        && !member.can_change_info
        && !member.can_invite_users
        && !member.can_pin_messages
        && !member.can_manage_topics
        && !member.can_react_to_messages
        && !member.can_edit_tag
}

fn keyboard(challenge: &CaptchaChallenge) -> InlineKeyboardMarkup {
    let buttons = challenge
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            InlineKeyboardButton::callback(option, format!("rc:{}:{index}", challenge.id))
        })
        .collect::<Vec<_>>();
    InlineKeyboardMarkup::new(vec![buttons[..2].to_vec(), buttons[2..].to_vec()])
}

fn generate_math_challenge(id: Uuid) -> MathChallenge {
    let bytes = id.as_bytes();
    let left = i16::from(bytes[0] % 9 + 1);
    let right = i16::from(bytes[1] % 9 + 1);
    let answer = left + right;
    let first_distractor = i16::from(bytes[2] % 17 + 2);
    let options = (0..17)
        .map(|offset| 2 + (first_distractor - 2 + offset) % 17)
        .filter(|value| *value != answer)
        .take(3)
        .fold(vec![answer], |mut values, value| {
            values.push(value);
            values
        });
    let rotation = usize::from(bytes[3] % 4);
    let mut options = options;
    options.rotate_left(rotation);
    let correct_option = options
        .iter()
        .position(|value| *value == answer)
        .unwrap_or_default() as i16;

    MathChallenge {
        id,
        question: format!("{left} + {right} = ?"),
        options: options.into_iter().map(|value| value.to_string()).collect(),
        correct_option,
    }
}

fn parse_callback_data(data: &str) -> Option<(Uuid, i16)> {
    let mut parts = data.split(':');
    if parts.next()? != "rc" {
        return None;
    }
    let id = Uuid::parse_str(parts.next()?).ok()?;
    let option = parts.next()?.parse::<i16>().ok()?;
    if parts.next().is_some() || !(0..4).contains(&option) {
        return None;
    }
    Some((id, option))
}

async fn create_or_load_challenge(
    pool: &PgPool,
    job: &NewUserAuditJob,
    risk_score: i32,
    math: &MathChallenge,
    restore_permissions: &Value,
) -> anyhow::Result<CaptchaChallenge> {
    let options = serde_json::to_value(&math.options)?;
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        r#"insert into telegram_risk_captcha_challenges
               (id, chat_id, telegram_user_id, audit_job_id, risk_score,
                question, options, correct_option, restore_permissions)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
           on conflict (chat_id, telegram_user_id) do nothing
           returning id"#,
    )
    .bind(math.id)
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .bind(job.id)
    .bind(risk_score)
    .bind(&math.question)
    .bind(options)
    .bind(math.correct_option)
    .bind(restore_permissions)
    .fetch_optional(&mut *tx)
    .await?;
    let challenge_id = if let Some(row) = inserted {
        row.try_get("id")?
    } else {
        sqlx::query_scalar(
            "select id from telegram_risk_captcha_challenges where chat_id = $1 and telegram_user_id = $2 for update",
        )
        .bind(job.chat_id)
        .bind(job.telegram_user_id)
        .fetch_one(&mut *tx)
        .await?
    };
    let row = select_challenge_by_id(&mut tx, challenge_id).await?;
    tx.commit().await?;
    decode_challenge(row)
}

async fn reopen_expired_challenge(
    pool: &PgPool,
    job: &NewUserAuditJob,
    risk_score: i32,
    math: &MathChallenge,
    restore_permissions: &Value,
) -> anyhow::Result<CaptchaChallenge> {
    let options = serde_json::to_value(&math.options)?;
    let updated_id = sqlx::query_scalar::<_, Uuid>(
        r#"update telegram_risk_captcha_challenges
           set id = $1, audit_job_id = $2, risk_score = $3, question = $4,
               options = $5, correct_option = $6, restore_permissions = $7,
               restriction_applied_at = null, message_id = null, expires_at = null,
               status = 'preparing', attempts = 0, setup_attempts = 0,
               setup_next_attempt_at = now(), setup_lease_expires_at = null,
               setup_error_kind = null, created_at = now(), updated_at = now(),
               solved_at = null
           where chat_id = $8 and telegram_user_id = $9 and status = 'expired'
           returning id"#,
    )
    .bind(math.id)
    .bind(job.id)
    .bind(risk_score)
    .bind(&math.question)
    .bind(options)
    .bind(restore_permissions)
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .fetch_optional(pool)
    .await?;
    match updated_id {
        Some(id) => load_challenge(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("reopened risk captcha challenge disappeared")),
        None => load_user_challenge(pool, job.chat_id, job.telegram_user_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("risk captcha challenge disappeared during renewal")),
    }
}

async fn claim_setup_by_id(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<CaptchaChallenge>> {
    let claimed_id = sqlx::query_scalar::<_, Uuid>(
        r#"update telegram_risk_captcha_challenges
           set status = 'setting_up', setup_attempts = setup_attempts + 1,
               setup_lease_expires_at = now() + ($2 * interval '1 second'),
               setup_error_kind = null, updated_at = now()
           where id = $1 and (
               (status = 'preparing' and setup_next_attempt_at <= now())
               or (status = 'pending' and message_id is null)
               or (status = 'setting_up' and setup_lease_expires_at <= now())
           )
           returning id"#,
    )
    .bind(id)
    .bind(SETUP_LEASE_SECONDS)
    .fetch_optional(pool)
    .await?;
    let Some(id) = claimed_id else {
        return Ok(None);
    };
    load_challenge(pool, id).await
}

async fn claim_next_setup(pool: &PgPool) -> anyhow::Result<Option<CaptchaChallenge>> {
    let row = sqlx::query(
        r#"with candidate as (
               select id
               from telegram_risk_captcha_challenges
               where (status = 'preparing' and setup_next_attempt_at <= now())
                  or (status = 'pending' and message_id is null)
                  or (status = 'setting_up' and setup_lease_expires_at <= now())
               order by setup_next_attempt_at, created_at, id
               for update skip locked
               limit 1
           )
           update telegram_risk_captcha_challenges challenge
           set status = 'setting_up', setup_attempts = setup_attempts + 1,
               setup_lease_expires_at = now() + ($1 * interval '1 second'),
               setup_error_kind = null, updated_at = now()
           from candidate
           where challenge.id = candidate.id
           returning challenge.id, challenge.chat_id, challenge.telegram_user_id,
                     challenge.question, challenge.options, challenge.correct_option,
                     challenge.restore_permissions, challenge.restriction_applied_at,
                     challenge.message_id, challenge.expires_at,
                     challenge.status, challenge.setup_attempts"#,
    )
    .bind(SETUP_LEASE_SECONDS)
    .fetch_optional(pool)
    .await?;
    row.map(decode_challenge).transpose()
}

async fn claim_next_expired(pool: &PgPool) -> anyhow::Result<Option<CaptchaChallenge>> {
    let row = sqlx::query(
        r#"with candidate as (
               select id
               from telegram_risk_captcha_challenges
               where (status in ('pending', 'failed') and expires_at <= now())
                  or (status = 'solving' and expires_at <= now()
                      and setup_lease_expires_at <= now())
                  or (status = 'expiring' and setup_lease_expires_at <= now())
               order by expires_at, created_at, id
               for update skip locked
               limit 1
           )
           update telegram_risk_captcha_challenges challenge
           set status = 'expiring',
               setup_lease_expires_at = now() + ($1 * interval '1 second'),
               updated_at = now()
           from candidate
           where challenge.id = candidate.id
           returning challenge.id, challenge.chat_id, challenge.telegram_user_id,
                     challenge.question, challenge.options, challenge.correct_option,
                     challenge.restore_permissions, challenge.restriction_applied_at,
                     challenge.message_id, challenge.expires_at,
                     challenge.status, challenge.setup_attempts"#,
    )
    .bind(SETUP_LEASE_SECONDS)
    .fetch_optional(pool)
    .await?;
    row.map(decode_challenge).transpose()
}

async fn expire_claimed_challenge(
    bot: &Bot,
    pool: &PgPool,
    challenge: &CaptchaChallenge,
) -> anyhow::Result<()> {
    #[cfg(feature = "manual-moderation")]
    if crate::features::manual_moderation::active_restriction_action(
        pool,
        challenge.chat_id,
        challenge.user_id,
    )
    .await?
    .is_some()
    {
        set_status(pool, challenge.id, "overridden").await?;
        update_captcha_card(bot, challenge, "Проверка отменена другим ограничением.").await;
        return Ok(());
    }

    let member = bot
        .get_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
        .await?;
    match member.kind {
        ChatMemberKind::Restricted(restricted)
            if restricted.is_member
                && has_no_permissions(&restricted)
                && challenge.restriction_applied_at.is_some() =>
        {
            bot.ban_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
                .await?;
            bot.unban_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
                .only_if_banned(true)
                .await?;
        }
        ChatMemberKind::Banned(_) if challenge.restriction_applied_at.is_some() => {
            bot.unban_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
                .only_if_banned(true)
                .await?;
        }
        ChatMemberKind::Left => {}
        _ => {
            set_status(pool, challenge.id, "overridden").await?;
            update_captcha_card(bot, challenge, "Проверка отменена модератором.").await;
            return Ok(());
        }
    }

    set_expired(pool, challenge.id).await?;
    update_captcha_card(bot, challenge, "Время вышло. Участник удалён из чата.").await;
    Ok(())
}

async fn update_captcha_card(bot: &Bot, challenge: &CaptchaChallenge, text: &str) {
    let Some(message_id) = challenge.message_id else {
        return;
    };
    if let Err(error) = bot
        .edit_message_text(
            ChatId(challenge.chat_id),
            teloxide::types::MessageId(message_id),
            text,
        )
        .reply_markup(InlineKeyboardMarkup::new(
            Vec::<Vec<InlineKeyboardButton>>::new(),
        ))
        .await
    {
        tracing::debug!(%error, chat_id = challenge.chat_id, user_id = challenge.user_id, "failed to update expired risk captcha card");
    }
}

async fn load_user_challenge(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<Option<CaptchaChallenge>> {
    let row = sqlx::query(
        "select id, chat_id, telegram_user_id, question, options, correct_option, restore_permissions, restriction_applied_at, message_id, expires_at, status, setup_attempts from telegram_risk_captcha_challenges where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    row.map(decode_challenge).transpose()
}

async fn load_challenge(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<CaptchaChallenge>> {
    let row = sqlx::query(
        "select id, chat_id, telegram_user_id, question, options, correct_option, restore_permissions, restriction_applied_at, message_id, expires_at, status, setup_attempts from telegram_risk_captcha_challenges where id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.map(decode_challenge).transpose()
}

async fn select_challenge_by_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<PgRow> {
    Ok(sqlx::query(
        "select id, chat_id, telegram_user_id, question, options, correct_option, restore_permissions, restriction_applied_at, message_id, expires_at, status, setup_attempts from telegram_risk_captcha_challenges where id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?)
}

fn decode_challenge(row: PgRow) -> anyhow::Result<CaptchaChallenge> {
    Ok(CaptchaChallenge {
        id: row.try_get("id")?,
        chat_id: row.try_get("chat_id")?,
        user_id: row.try_get("telegram_user_id")?,
        question: row.try_get("question")?,
        options: serde_json::from_value(row.try_get("options")?)?,
        correct_option: row.try_get("correct_option")?,
        restore_permissions: serde_json::from_value(row.try_get("restore_permissions")?)?,
        restriction_applied_at: row.try_get("restriction_applied_at")?,
        message_id: row.try_get("message_id")?,
        expires_at: row.try_get("expires_at")?,
        status: row.try_get("status")?,
        setup_attempts: row.try_get("setup_attempts")?,
    })
}

async fn set_message_sent(
    pool: &PgPool,
    id: Uuid,
    message_id: i32,
    ttl_seconds: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "update telegram_risk_captcha_challenges set status = 'pending', message_id = coalesce(message_id, $2), expires_at = coalesce(expires_at, now() + ($3 * interval '1 second')), setup_lease_expires_at = null, setup_error_kind = null, updated_at = now() where id = $1 and status in ('preparing', 'setting_up', 'pending')",
    )
    .bind(id)
    .bind(message_id)
    .bind(ttl_seconds)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_restriction_applied(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "update telegram_risk_captcha_challenges set restriction_applied_at = coalesce(restriction_applied_at, now()), updated_at = now() where id = $1 and status in ('setting_up', 'pending', 'solving')",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn set_status(pool: &PgPool, id: Uuid, status: &str) -> anyhow::Result<()> {
    sqlx::query(
        "update telegram_risk_captcha_challenges set status = $2, setup_lease_expires_at = null, updated_at = now() where id = $1 and status not in ('passed', 'failed', 'overridden', 'setup_failed')",
    )
    .bind(id)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(())
}

async fn set_expired(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "update telegram_risk_captcha_challenges set status = 'expired', setup_lease_expires_at = null, updated_at = now() where id = $1 and status = 'expiring'",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Фиксирует завершение только после успешного восстановления Telegram-прав.
pub async fn set_passed(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "update telegram_risk_captcha_challenges set status = 'passed', setup_lease_expires_at = null, solved_at = now(), updated_at = now() where id = $1 and status in ('preparing', 'setting_up', 'pending', 'solving')",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Захватывает переход к снятию ограничений; повторный клик безопасно
/// повторяет API-вызов после временной ошибки или рестарта процесса.
pub async fn mark_solving(pool: &PgPool, id: Uuid) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "update telegram_risk_captcha_challenges set status = 'solving', setup_lease_expires_at = now() + ($2 * interval '1 second'), updated_at = now() where id = $1 and status in ('preparing', 'setting_up', 'pending', 'solving') and (expires_at is null or expires_at > now())",
    )
    .bind(id)
    .bind(SETUP_LEASE_SECONDS)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Атомарно учитывает неверный ответ и закрывает challenge после трёх ошибок.
pub async fn record_wrong_answer(
    pool: &PgPool,
    id: Uuid,
) -> anyhow::Result<CaptchaCallbackOutcome> {
    let row = sqlx::query(
        r#"update telegram_risk_captcha_challenges
           set attempts = attempts + 1,
               status = case when attempts + 1 >= $2 then 'failed' else 'pending' end,
               updated_at = now()
           where id = $1 and status in ('preparing', 'setting_up', 'pending')
             and (expires_at is null or expires_at > now())
           returning attempts, status"#,
    )
    .bind(id)
    .bind(MAX_WRONG_ANSWERS)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(CaptchaCallbackOutcome::Stale);
    };
    let attempts: i32 = row.try_get("attempts")?;
    let status: String = row.try_get("status")?;
    if status == "failed" {
        Ok(CaptchaCallbackOutcome::Failed)
    } else {
        Ok(CaptchaCallbackOutcome::Wrong {
            attempts_left: MAX_WRONG_ANSWERS - attempts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{generate_math_challenge, is_callback_data, parse_callback_data};
    use uuid::Uuid;

    #[test]
    fn generated_math_captcha_has_four_distinct_options_and_one_correct_answer() {
        for byte in 0..=u8::MAX {
            let mut bytes = [byte; 16];
            bytes[6] = (bytes[6] & 0x0f) | 0x40;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            let challenge = generate_math_challenge(Uuid::from_bytes(bytes));
            assert_eq!(challenge.options.len(), 4);
            assert_eq!(
                challenge
                    .options
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                4
            );
            let (left, right) = challenge
                .question
                .strip_suffix(" = ?")
                .unwrap()
                .split_once(" + ")
                .unwrap();
            let answer = left.parse::<i16>().unwrap() + right.parse::<i16>().unwrap();
            assert_eq!(
                challenge.options[challenge.correct_option as usize]
                    .parse::<i16>()
                    .unwrap(),
                answer
            );
        }
    }

    #[test]
    fn captcha_callback_data_is_bound_to_a_valid_option_index() {
        let id = Uuid::new_v4();
        let data = format!("rc:{id}:3");
        assert!(is_callback_data(&data));
        assert_eq!(parse_callback_data(&data), Some((id, 3)));
        assert_eq!(parse_callback_data(&format!("rc:{id}:4")), None);
        assert_eq!(parse_callback_data("other:uuid:1"), None);
    }
}
