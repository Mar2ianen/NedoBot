use serde_json::Value;
use sqlx::{PgPool, Row, postgres::PgRow};
use teloxide::{
    payloads::{
        EditEphemeralMessageReplyMarkupSetters, EditEphemeralMessageTextSetters,
        RestrictChatMemberSetters, SendMessageSetters, UnbanChatMemberSetters,
    },
    prelude::{Bot, Requester},
    types::{
        ChatId, ChatMemberKind, ChatPermissions, EphemeralMessageParameters, InlineKeyboardButton,
        InlineKeyboardMarkup, MessageId, ReplyParameters, UserId,
    },
};
use uuid::Uuid;

use crate::features::new_user_audit::repo::NewUserAuditJob;

const MAX_WRONG_ANSWERS: i32 = 3;
const MAX_SETUP_ATTEMPTS: i32 = 5;
const SETUP_LEASE_SECONDS: i64 = 120;
const TELEGRAM_DELETE_BATCH_SIZE: usize = 100;
const MAX_CAPTCHA_CLEANUP_MESSAGES: i64 = 1_000;

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
    PassedWithoutRestriction,
    PassedButOtherRestrictionRemains,
    Wrong { attempts_left: i32 },
    Failed,
    Expired,
    Stale,
    NotForThisUser,
}

impl CaptchaCallbackOutcome {
    pub fn success_message(self) -> Option<&'static str> {
        match self {
            Self::Passed => Some("Проверка пройдена. Ограничения сняты."),
            Self::PassedWithoutRestriction => Some("Проверка пройдена."),
            Self::PassedButOtherRestrictionRemains => {
                Some("Проверка пройдена. Другое ограничение остаётся.")
            }
            Self::Wrong { .. }
            | Self::Failed
            | Self::Expired
            | Self::Stale
            | Self::NotForThisUser => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Passed
                | Self::PassedWithoutRestriction
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
    ephemeral_message_id: Option<i32>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    status: String,
    setup_attempts: i32,
    decision_job_id: Option<i64>,
    ban_on_failure: bool,
}

enum VerificationState {
    RestrictedMember,
    Commenter,
}

async fn can_offer_challenge(
    pool: &PgPool,
    member: &teloxide::types::ChatMember,
    chat_id: i64,
    user_id: i64,
    decision_job_id: Option<i64>,
) -> anyhow::Result<bool> {
    anyhow::ensure!(
        member.user.id.0 as i64 == user_id,
        "captcha member response has a different user"
    );
    let membership_context = crate::db::membership::observe_member(pool, chat_id, member).await?;
    tracing::debug!(
        chat_id,
        user_id,
        ?membership_context,
        "risk captcha membership context"
    );
    Ok(match &member.kind {
        ChatMemberKind::Member(_) => true,
        // Left не означает отсутствие комментариев в linked discussion.
        // Случайному non-member без своего свежего сообщения проверку не шлём.
        ChatMemberKind::Left => {
            if let Some(id) = decision_job_id {
                sqlx::query_scalar("select exists(select 1 from moderation_decision_jobs j join telegram_messages m on m.chat_id=j.chat_id and m.message_id=j.message_id and m.user_id=j.user_id where j.id=$1 and j.chat_id=$2 and j.user_id=$3 and m.created_at>now()-interval '24 hours' and m.deleted_by_bot_at is null)")
                .bind(id).bind(chat_id).bind(user_id).fetch_one(pool).await?
            } else {
                load_reply_message_id(pool, chat_id, user_id)
                    .await?
                    .is_some()
            }
        }
        _ => false,
    })
}

struct CaptchaRequest {
    chat_id: i64,
    telegram_user_id: i64,
    audit_job_id: Option<i64>,
    decision_job_id: Option<i64>,
    ban_on_failure: bool,
}

pub async fn apply_decision_captcha(
    bot: &Bot,
    pool: &PgPool,
    job: &crate::features::decision_audit::repo::DecisionJob,
    risk_score: i32,
    ttl_seconds: i64,
    ban_on_failure: bool,
) -> anyhow::Result<CaptchaOutcome> {
    if !crate::features::decision_audit::repo::captcha_source_is_current(
        pool,
        job.id,
        job.chat_id,
        job.user_id,
    )
    .await?
    {
        return Ok(CaptchaOutcome::Overridden);
    }
    apply_captcha(
        bot,
        pool,
        &CaptchaRequest {
            chat_id: job.chat_id,
            telegram_user_id: job.user_id,
            audit_job_id: None,
            decision_job_id: Some(job.id),
            ban_on_failure,
        },
        risk_score,
        ttl_seconds,
    )
    .await
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
    apply_captcha(
        bot,
        pool,
        &CaptchaRequest {
            chat_id: job.chat_id,
            telegram_user_id: job.telegram_user_id,
            audit_job_id: Some(job.id),
            decision_job_id: None,
            ban_on_failure: false,
        },
        risk_score,
        ttl_seconds,
    )
    .await
}

async fn apply_captcha(
    bot: &Bot,
    pool: &PgPool,
    job: &CaptchaRequest,
    risk_score: i32,
    ttl_seconds: i64,
) -> anyhow::Result<CaptchaOutcome> {
    let existing = load_user_challenge(pool, job.chat_id, job.telegram_user_id).await?;
    if let Some(challenge) = existing.as_ref() {
        decision_source_valid(bot, pool, challenge).await?;
    }
    let challenge = match load_user_challenge(pool, job.chat_id, job.telegram_user_id).await? {
        Some(challenge)
            if challenge.status == "expired"
                || (challenge.status == "overridden"
                    && challenge.decision_job_id.is_some()
                    && job.decision_job_id.is_some()) =>
        {
            let member = bot
                .get_chat_member(ChatId(job.chat_id), UserId(job.telegram_user_id as u64))
                .await?;
            if !can_offer_challenge(
                pool,
                &member,
                job.chat_id,
                job.telegram_user_id,
                job.decision_job_id,
            )
            .await?
            {
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
            if !can_offer_challenge(
                pool,
                &member,
                job.chat_id,
                job.telegram_user_id,
                job.decision_job_id,
            )
            .await?
            {
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
            if ensure_verification_state(bot, pool, &challenge)
                .await?
                .is_none()
            {
                return Ok(CaptchaOutcome::Overridden);
            }
            delete_audited_messages(bot, pool, &challenge).await?;
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
    if !decision_source_valid(bot, pool, &challenge).await? {
        return Ok(true);
    }
    if challenge.status == "pending" && challenge.message_id.is_some() {
        // Cleanup retry не должен накладывать новый mute, если автор успел
        // вступить в группу, пройти проверку или был размучен администратором.
        let member = bot
            .get_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
            .await?;
        let unchanged = match member.kind {
            ChatMemberKind::Left => challenge.restriction_applied_at.is_none(),
            ChatMemberKind::Restricted(restricted) => {
                restricted.is_member
                    && has_no_permissions(&restricted)
                    && challenge.restriction_applied_at.is_some()
            }
            _ => false,
        };
        if unchanged {
            cleanup_after_delivery(bot, pool, &challenge).await?;
        } else {
            set_status(pool, challenge.id, "overridden").await?;
        }
        return Ok(true);
    }
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
    let reply_message_id = if challenge.decision_job_id.is_some() {
        decision_message_ids(pool, challenge, 1)
            .await?
            .first()
            .copied()
    } else {
        load_reply_message_id(pool, challenge.chat_id, challenge.user_id).await?
    };
    let Some(state) = ensure_verification_state(bot, pool, challenge).await? else {
        return Ok(CaptchaOutcome::Overridden);
    };
    anyhow::ensure!(
        reply_message_id.is_some() || matches!(state, VerificationState::RestrictedMember),
        "non-member captcha requires a fresh comment reply target"
    );

    let consequence = match (challenge.ban_on_failure, state) {
        (true, VerificationState::RestrictedMember) => {
            "До успешного ответа отправка сообщений ограничена; если проверка не пройдена вовремя, доступ к чату будет заблокирован."
        }
        (true, VerificationState::Commenter) => {
            "После успешного ответа проверка будет завершена; если её не пройти вовремя, доступ к комментариям будет заблокирован."
        }
        (false, VerificationState::RestrictedMember) => {
            "До успешного ответа отправка сообщений ограничена; после истечения времени бот удалит вас из чата."
        }
        (false, VerificationState::Commenter) => "После успешного ответа проверка будет завершена.",
    };
    let text = format!(
        "Проверка комментария\nРешите пример: {}.\nУ вас {} мин. {consequence}",
        challenge.question,
        ttl_seconds.saturating_add(59) / 60
    );
    let sent = with_captcha_ephemeral_parameters(
        bot.send_message(text).chat_id(ChatId(challenge.chat_id)),
        UserId(challenge.user_id as u64),
        reply_message_id,
    );
    let sent = sent.reply_markup(keyboard(challenge)).await?;
    // Успех sendMessage не является read receipt. Но отсутствие ephemeral ID
    // не позволяет активировать проверку или удалять исходный комментарий.
    anyhow::ensure!(
        sent.ephemeral_message_id.is_some(),
        "captcha response is not ephemeral"
    );
    set_message_sent(
        pool,
        challenge.id,
        sent.id.0,
        sent.ephemeral_message_id,
        ttl_seconds,
    )
    .await?;
    if challenge.decision_job_id.is_some() {
        let delivered = load_challenge(pool, challenge.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("captcha disappeared after delivery"))?;
        if !decision_source_valid(bot, pool, &delivered).await? {
            return Ok(CaptchaOutcome::Overridden);
        }
    }
    // Reply должен быть принят до удаления anchor. Ошибка cleanup не должна
    // переотправлять уже сохранённую карточку или продлевать её TTL.
    cleanup_after_delivery(bot, pool, challenge).await?;
    Ok(CaptchaOutcome::Issued)
}

async fn cleanup_after_delivery(
    bot: &Bot,
    pool: &PgPool,
    challenge: &CaptchaChallenge,
) -> anyhow::Result<()> {
    let failed = challenge.setup_attempts > MAX_SETUP_ATTEMPTS
        || delete_audited_messages(bot, pool, challenge).await.is_err();
    let error_kind = if !failed {
        None
    } else if challenge.setup_attempts < MAX_SETUP_ATTEMPTS {
        Some("cleanup_pending")
    } else {
        Some("cleanup_failed")
    };
    // Карточка уже принята: ошибка cleanup не переводит её обратно в setup
    // и не восстанавливает права. Воркер повторит только удаление, bounded.
    sqlx::query("update telegram_risk_captcha_challenges set setup_error_kind=$3, setup_next_attempt_at=now()+($4*interval '1 second'), setup_lease_expires_at=null, updated_at=now() where id=$1 and status='pending' and setup_attempts=$2")
        .bind(challenge.id).bind(challenge.setup_attempts).bind(error_kind)
        .bind(setup_retry_delay_seconds(challenge.setup_attempts)).execute(pool).await?;
    if failed {
        tracing::warn!(
            chat_id = challenge.chat_id,
            user_id = challenge.user_id,
            error_kind,
            "risk captcha cleanup deferred after ephemeral delivery"
        );
    }
    Ok(())
}

fn setup_retry_delay_seconds(attempts: i32) -> i64 {
    2_i64.pow(attempts.clamp(1, 6) as u32).min(60)
}

async fn load_reply_message_id(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<Option<i32>> {
    Ok(sqlx::query_scalar(
        r#"select m.message_id from telegram_messages m
           join telegram_new_user_profile_audits a
             on a.chat_id=m.chat_id and a.telegram_user_id=m.user_id
           where m.chat_id=$1 and m.user_id=$2
             and m.message_id between a.first_message_id and a.last_message_id
             and m.source_channel_id is null and m.deleted_by_bot_at is null
             and m.created_at >= now() - ($3 * interval '1 second')
             and (nullif(btrim(m.text), '') is not null
                  or m.has_photo or m.has_video or m.has_document or m.has_audio
                  or m.has_voice or m.has_sticker or m.has_animation
                  or m.raw_json ?| array['poll','video_note','contact','location',
                                        'venue','dice','paid_media','story','game'])
           order by m.message_id desc limit 1"#,
    )
    .bind(chat_id)
    .bind(user_id)
    .bind(crate::features::auto_moderation::ENFORCE_FRESHNESS.num_seconds())
    .fetch_optional(pool)
    .await?)
}

/// После мута очищаем всю свежую пачку пользователя в границах аудита.
/// Пакетный Telegram-метод пропускает уже удалённые модератором сообщения;
/// подтверждённые пачки отмечаются в БД и не отправляются повторно при retry.
async fn delete_audited_messages(
    bot: &Bot,
    pool: &PgPool,
    challenge: &CaptchaChallenge,
) -> anyhow::Result<()> {
    let message_ids: Vec<i32> = if challenge.decision_job_id.is_some() {
        decision_message_ids(pool, challenge, MAX_CAPTCHA_CLEANUP_MESSAGES).await?
    } else {
        sqlx::query_scalar(
            r#"select m.message_id
           from telegram_messages m
           join telegram_new_user_profile_audits a
             on a.chat_id = m.chat_id and a.telegram_user_id = m.user_id
           where a.chat_id = $1 and a.telegram_user_id = $2
             and m.message_id between a.first_message_id and a.last_message_id
             and m.source_channel_id is null and m.deleted_by_bot_at is null
             and m.created_at >= now() - ($3 * interval '1 second')
             and (nullif(btrim(m.text), '') is not null
                  or m.has_photo or m.has_video or m.has_document or m.has_audio
                  or m.has_voice or m.has_sticker or m.has_animation
                  or m.raw_json ?| array['poll', 'video_note', 'contact', 'location',
                                        'venue', 'dice', 'paid_media', 'story', 'game'])
           order by m.message_id desc limit $4"#,
        )
        .bind(challenge.chat_id)
        .bind(challenge.user_id)
        .bind(crate::features::auto_moderation::ENFORCE_FRESHNESS.num_seconds())
        .bind(MAX_CAPTCHA_CLEANUP_MESSAGES)
        .fetch_all(pool)
        .await?
    };
    for batch in message_ids.chunks(TELEGRAM_DELETE_BATCH_SIZE) {
        bot.delete_messages(
            ChatId(challenge.chat_id),
            batch.iter().copied().map(MessageId),
        )
        .await?;
        sqlx::query(
            "update telegram_messages set deleted_by_bot_at = now(), deleted_by_bot_actor_id = null, deleted_by_bot_reason = 'risk_captcha_cleanup', updated_at = now() where chat_id = $1 and user_id = $2 and message_id = any($3) and deleted_by_bot_at is null",
        )
        .bind(challenge.chat_id)
        .bind(challenge.user_id)
        .bind(batch)
        .execute(pool)
        .await?;
    }
    if !message_ids.is_empty() {
        tracing::info!(
            chat_id = challenge.chat_id,
            user_id = challenge.user_id,
            deleted_messages = message_ids.len(),
            "risk captcha audit messages cleaned"
        );
    }
    if message_ids.len() == MAX_CAPTCHA_CLEANUP_MESSAGES as usize {
        tracing::warn!(
            chat_id = challenge.chat_id,
            user_id = challenge.user_id,
            "risk captcha cleanup reached its message budget"
        );
    }
    Ok(())
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

    let delay_seconds = setup_retry_delay_seconds(challenge.setup_attempts);
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
    if !decision_source_valid(bot, pool, &challenge).await? {
        return Ok(CaptchaCallbackOutcome::Stale);
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
        ChatMemberKind::Left if challenge.restriction_applied_at.is_none() => {
            set_passed(pool, challenge.id).await?;
            Ok(CaptchaCallbackOutcome::PassedWithoutRestriction)
        }
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

async fn ensure_verification_state(
    bot: &Bot,
    pool: &PgPool,
    challenge: &CaptchaChallenge,
) -> anyhow::Result<Option<VerificationState>> {
    if !decision_source_valid(bot, pool, challenge).await? {
        return Ok(None);
    }
    let member = bot
        .get_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
        .await?;
    match member.kind {
        ChatMemberKind::Member(_) => {
            if challenge.restriction_applied_at.is_some() {
                set_status(pool, challenge.id, "overridden").await?;
                return Ok(None);
            }
            bot.restrict_chat_member(
                ChatId(challenge.chat_id),
                UserId(challenge.user_id as u64),
                ChatPermissions::empty(),
            )
            .use_independent_chat_permissions(true)
            .await?;
            mark_restriction_applied(pool, challenge.id).await?;
            Ok(Some(VerificationState::RestrictedMember))
        }
        ChatMemberKind::Restricted(restricted)
            if restricted.is_member && has_no_permissions(&restricted) =>
        {
            if challenge.restriction_applied_at.is_none() {
                set_status(pool, challenge.id, "overridden").await?;
                return Ok(None);
            }
            Ok(Some(VerificationState::RestrictedMember))
        }
        ChatMemberKind::Left if challenge.restriction_applied_at.is_none() => {
            // Не пытаемся мутить/вступать за non-member. Доставка ephemeral
            // reply проверяется отдельно; отказ API не запускает TTL.
            Ok(Some(VerificationState::Commenter))
        }
        _ => {
            set_status(pool, challenge.id, "overridden").await?;
            Ok(None)
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
        ChatMemberKind::Left if challenge.restriction_applied_at.is_none() => Ok(true),
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
    job: &CaptchaRequest,
    risk_score: i32,
    math: &MathChallenge,
    restore_permissions: &Value,
) -> anyhow::Result<CaptchaChallenge> {
    let options = serde_json::to_value(&math.options)?;
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        r#"insert into telegram_risk_captcha_challenges
               (id, chat_id, telegram_user_id, audit_job_id, risk_score,
                question, options, correct_option, restore_permissions,decision_job_id,ban_on_failure)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9,$10,$11)
           on conflict (chat_id, telegram_user_id) do nothing
           returning id"#,
    )
    .bind(math.id)
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .bind(job.audit_job_id)
    .bind(risk_score)
    .bind(&math.question)
    .bind(options)
    .bind(math.correct_option)
    .bind(restore_permissions)
    .bind(job.decision_job_id)
    .bind(job.ban_on_failure)
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
    job: &CaptchaRequest,
    risk_score: i32,
    math: &MathChallenge,
    restore_permissions: &Value,
) -> anyhow::Result<CaptchaChallenge> {
    let options = serde_json::to_value(&math.options)?;
    let updated_id = sqlx::query_scalar::<_, Uuid>(
        r#"update telegram_risk_captcha_challenges
           set id = $1, audit_job_id = $2, risk_score = $3, question = $4,decision_job_id=$10,ban_on_failure=$11,
               options = $5, correct_option = $6, restore_permissions = $7,
               restriction_applied_at = null, message_id = null,
               ephemeral_message_id = null, expires_at = null,
               status = 'preparing', attempts = 0, setup_attempts = 0,
               setup_next_attempt_at = now(), setup_lease_expires_at = null,
               setup_error_kind = null, created_at = now(), updated_at = now(),
               solved_at = null
           where chat_id = $8 and telegram_user_id = $9 and (status='expired' or (status='overridden' and setup_error_kind='decision_source_changed' and $10::bigint is not null))
           returning id"#,
    )
    .bind(math.id)
    .bind(job.audit_job_id)
    .bind(risk_score)
    .bind(&math.question)
    .bind(options)
    .bind(restore_permissions)
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .bind(job.decision_job_id)
    .bind(job.ban_on_failure)
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
               select id, status, message_id
               from telegram_risk_captcha_challenges
               where (status = 'preparing' and setup_next_attempt_at <= now())
                  or (status = 'pending' and message_id is null)
                  or (status = 'setting_up' and setup_lease_expires_at <= now())
                  or (status = 'pending' and message_id is not null
                      and setup_error_kind in ('cleanup_pending','decision_recheck')
                      and setup_next_attempt_at <= now() and expires_at > now()
                      and (setup_lease_expires_at is null or setup_lease_expires_at <= now()))
               order by setup_next_attempt_at, created_at, id
               for update skip locked
               limit 1
           )
           update telegram_risk_captcha_challenges challenge
           set status = case when candidate.status = 'pending' and candidate.message_id is not null
                             then 'pending' else 'setting_up' end,
               setup_attempts = setup_attempts + 1,
               setup_lease_expires_at = now() + ($1 * interval '1 second'),
               setup_error_kind = case when candidate.status = 'pending' and candidate.message_id is not null
                                       then 'cleanup_pending' else null end,
               updated_at = now()
           from candidate
           where challenge.id = candidate.id
           returning challenge.id, challenge.chat_id, challenge.telegram_user_id,
                     challenge.question, challenge.options, challenge.correct_option,
                     challenge.restore_permissions, challenge.restriction_applied_at,
                     challenge.message_id, challenge.ephemeral_message_id,
                     challenge.expires_at,
                     challenge.status, challenge.setup_attempts,challenge.decision_job_id,challenge.ban_on_failure"#,
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
                     challenge.message_id, challenge.ephemeral_message_id,
                     challenge.expires_at,
                     challenge.status, challenge.setup_attempts,challenge.decision_job_id,challenge.ban_on_failure"#,
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
        remove_captcha_card(bot, challenge).await;
        return Ok(());
    }

    if !decision_source_valid(bot, pool, challenge).await? {
        return Ok(());
    }
    let member = bot
        .get_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
        .await?;
    if challenge.ban_on_failure {
        let may_ban = match &member.kind {
            ChatMemberKind::Restricted(r) => {
                r.is_member && has_no_permissions(r) && challenge.restriction_applied_at.is_some()
            }
            ChatMemberKind::Left => true,
            ChatMemberKind::Member(_) => challenge.restriction_applied_at.is_none(),
            ChatMemberKind::Banned(_) => false,
            _ => {
                set_status(pool, challenge.id, "overridden").await?;
                remove_captcha_card(bot, challenge).await;
                return Ok(());
            }
        };
        if may_ban {
            // Старый source/normal label проверяются повторно непосредственно перед баном.
            if !decision_source_valid(bot, pool, challenge).await? {
                return Ok(());
            }
            bot.ban_chat_member(ChatId(challenge.chat_id), UserId(challenge.user_id as u64))
                .await?;
        }
        set_expired(pool, challenge.id).await?;
        tracing::warn!(chat_id=challenge.chat_id,user_id=challenge.user_id,challenge_id=%challenge.id,decision_job_id=challenge.decision_job_id,"Decision captcha failed; chat access blocked");
        remove_captcha_card(bot, challenge).await;
        return Ok(());
    }
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
            remove_captcha_card(bot, challenge).await;
            return Ok(());
        }
    }

    set_expired(pool, challenge.id).await?;
    remove_captcha_card(bot, challenge).await;
    Ok(())
}

async fn remove_captcha_card(bot: &Bot, challenge: &CaptchaChallenge) {
    let result = match (challenge.ephemeral_message_id, challenge.message_id) {
        (Some(ephemeral_message_id), _) => {
            bot.delete_ephemeral_message(
                ChatId(challenge.chat_id),
                UserId(challenge.user_id as u64),
                ephemeral_message_id,
            )
            .await
        }
        (None, Some(message_id)) => {
            bot.delete_message(ChatId(challenge.chat_id), MessageId(message_id))
                .await
        }
        (None, None) => return,
    };
    if let Err(error) = result {
        tracing::debug!(%error, chat_id = challenge.chat_id, user_id = challenge.user_id, "failed to remove completed risk captcha message");
    }
}

pub async fn finish_captcha_card(
    bot: &Bot,
    chat_id: ChatId,
    user_id: UserId,
    message_id: MessageId,
    ephemeral_message_id: Option<i32>,
    success_text: Option<&str>,
) {
    let empty_keyboard = InlineKeyboardMarkup::new(Vec::<Vec<InlineKeyboardButton>>::new());
    let result = match (ephemeral_message_id, success_text) {
        (Some(ephemeral_message_id), Some(text)) => bot
            .edit_ephemeral_message_text(chat_id, user_id, ephemeral_message_id)
            .text(text)
            .reply_markup(empty_keyboard)
            .await
            .map(|_| ()),
        (Some(ephemeral_message_id), None) => bot
            .edit_ephemeral_message_reply_markup(chat_id, user_id, ephemeral_message_id)
            .reply_markup(empty_keyboard)
            .await
            .map(|_| ()),
        (None, _) => bot.delete_message(chat_id, message_id).await.map(|_| ()),
    };
    if let Err(error) = result {
        tracing::debug!(%error, chat_id = chat_id.0, user_id = user_id.0, "failed to finish risk captcha message");
    }
}

fn with_captcha_ephemeral_parameters<R: SendMessageSetters>(
    request: R,
    user_id: UserId,
    reply_message_id: Option<i32>,
) -> R {
    let request = request.ephemeral_message_parameters(EphemeralMessageParameters::new(user_id));
    match reply_message_id {
        Some(id) => request.reply_parameters(ReplyParameters::new(MessageId(id))),
        None => request,
    }
}

async fn load_user_challenge(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<Option<CaptchaChallenge>> {
    let row = sqlx::query(
        "select id, chat_id, telegram_user_id, question, options, correct_option, restore_permissions, restriction_applied_at, message_id, ephemeral_message_id, expires_at, status, setup_attempts,decision_job_id,ban_on_failure from telegram_risk_captcha_challenges where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    row.map(decode_challenge).transpose()
}

async fn load_challenge(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<CaptchaChallenge>> {
    let row = sqlx::query(
        "select id, chat_id, telegram_user_id, question, options, correct_option, restore_permissions, restriction_applied_at, message_id, ephemeral_message_id, expires_at, status, setup_attempts,decision_job_id,ban_on_failure from telegram_risk_captcha_challenges where id = $1",
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
        "select id, chat_id, telegram_user_id, question, options, correct_option, restore_permissions, restriction_applied_at, message_id, ephemeral_message_id, expires_at, status, setup_attempts,decision_job_id,ban_on_failure from telegram_risk_captcha_challenges where id = $1",
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
        ephemeral_message_id: row.try_get("ephemeral_message_id")?,
        expires_at: row.try_get("expires_at")?,
        status: row.try_get("status")?,
        setup_attempts: row.try_get("setup_attempts")?,
        decision_job_id: row.try_get("decision_job_id")?,
        ban_on_failure: row.try_get("ban_on_failure")?,
    })
}

async fn set_message_sent(
    pool: &PgPool,
    id: Uuid,
    message_id: i32,
    ephemeral_message_id: Option<i32>,
    ttl_seconds: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "update telegram_risk_captcha_challenges set status = 'pending', message_id = coalesce(message_id, $2), ephemeral_message_id = coalesce(ephemeral_message_id, $3), expires_at = coalesce(expires_at, now() + ($4 * interval '1 second')), setup_lease_expires_at = null, setup_error_kind = null, updated_at = now() where id = $1 and status in ('preparing', 'setting_up', 'pending')",
    )
    .bind(id)
    .bind(message_id)
    .bind(ephemeral_message_id)
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

async fn decision_source_valid(
    bot: &Bot,
    pool: &PgPool,
    c: &CaptchaChallenge,
) -> anyhow::Result<bool> {
    if matches!(c.status.as_str(), "passed" | "expired") {
        return Ok(true);
    }
    if matches!(c.status.as_str(), "overridden" | "setup_failed") {
        return Ok(false);
    }
    let Some(id) = c.decision_job_id else {
        return Ok(true);
    };
    if crate::features::decision_audit::repo::captcha_source_is_current(
        pool, id, c.chat_id, c.user_id,
    )
    .await?
    {
        return Ok(true);
    }
    restore_after_failed_delivery(bot, pool, c).await?;
    sqlx::query("update telegram_risk_captcha_challenges set status='overridden',setup_error_kind='decision_source_changed',setup_lease_expires_at=null,updated_at=now() where id=$1 and status not in ('passed','expired')")
        .bind(c.id).execute(pool).await?;
    remove_captcha_card(bot, c).await;
    Ok(false)
}

/// Decision удаляет только собственные проверенные кандидаты, не соседнюю беседу.
async fn decision_message_ids(
    pool: &PgPool,
    c: &CaptchaChallenge,
    limit: i64,
) -> anyhow::Result<Vec<i32>> {
    Ok(sqlx::query_scalar(r#"select distinct m.message_id from telegram_messages m
        join moderation_decision_jobs j on j.chat_id=m.chat_id and j.message_id=m.message_id and j.user_id=m.user_id
        where m.chat_id=$1 and m.user_id=$2 and m.source_channel_id is null and m.sender_chat_id is null
          and m.deleted_by_bot_at is null and j.status='succeeded' and j.shadow_verdict='review_candidate'
          and j.source_edited_at is not distinct from m.edited_at
          and (j.id=$3 or j.metrics_json->'captcha_authorized'='true'::jsonb)
          and m.created_at>now()-interval '24 hours'
          and not exists(select 1 from moderation_decision_jobs newer where newer.chat_id=j.chat_id
              and newer.message_id=j.message_id and newer.user_id=j.user_id and newer.id>j.id
              and newer.status='succeeded' and newer.shadow_verdict<>'review_candidate')
        order by m.message_id desc limit $4"#)
        .bind(c.chat_id).bind(c.user_id).bind(c.decision_job_id).bind(limit).fetch_all(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::{
        CaptchaCallbackOutcome, generate_math_challenge, is_callback_data, parse_callback_data,
        with_captcha_ephemeral_parameters,
    };
    use teloxide::{
        payloads::SendMessageSetters,
        prelude::{Bot, ChatId, Requester},
        requests::HasPayload,
        types::UserId,
    };
    use uuid::Uuid;

    #[test]
    fn captcha_message_is_visible_only_to_its_challenged_user() {
        let recipient = UserId(42);
        let request = with_captcha_ephemeral_parameters(
            Bot::new("123456:TEST_TOKEN")
                .send_message("solve this")
                .chat_id(ChatId(-1001)),
            recipient,
            Some(17),
        );

        assert_eq!(
            request
                .payload_ref()
                .ephemeral_message_parameters
                .as_ref()
                .and_then(|parameters| parameters.receiver_user_id),
            Some(recipient)
        );
        let reply = request.payload_ref().reply_parameters.as_ref().unwrap();
        assert_eq!(reply.message_id, Some(teloxide::types::MessageId(17)));
        assert_ne!(reply.allow_sending_without_reply, Some(true));
        assert!(reply.chat_id.is_none());
    }

    #[test]
    fn only_successful_captcha_outcomes_have_visible_result_text() {
        assert!(CaptchaCallbackOutcome::Passed.success_message().is_some());
        assert!(
            CaptchaCallbackOutcome::PassedButOtherRestrictionRemains
                .success_message()
                .is_some()
        );
        for outcome in [
            CaptchaCallbackOutcome::Wrong { attempts_left: 2 },
            CaptchaCallbackOutcome::Failed,
            CaptchaCallbackOutcome::Expired,
            CaptchaCallbackOutcome::Stale,
            CaptchaCallbackOutcome::NotForThisUser,
        ] {
            assert_eq!(outcome.success_message(), None);
        }
    }

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
