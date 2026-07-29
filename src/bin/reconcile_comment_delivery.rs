use anyhow::Context;
use teloxide::{Bot, prelude::RequesterExt, types::ParseMode};
use tg_ai_bot_teloxide::{
    config::Config,
    db::build_pool,
    features::first_comment::{
        pipeline::process_claimed_post_comment_job,
        repo::{
            OperatorAuditParams, claim_delivery_unknown_post_comment_for_operator_retry,
            inspect_post_comment_job, list_delivery_unknown_post_comment_jobs,
            mark_delivery_unknown_post_comment_delivered,
            mark_delivery_unknown_post_comment_failed,
            mark_operator_retry_post_comment_terminal_failed,
        },
    },
    state::AppState,
};

const MAX_LIST_LIMIT: i64 = 100;

#[derive(Debug)]
enum Command {
    List {
        limit: i64,
    },
    Inspect {
        job_id: i64,
    },
    MarkDelivered {
        job_id: i64,
        bot_comment_message_id: i32,
        actor: String,
        reason: String,
    },
    MarkFailed {
        job_id: i64,
        actor: String,
        reason: String,
    },
    Retry {
        job_id: i64,
        actor: String,
        reason: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let command = parse_args()?;
    let pool = build_pool().await?;

    match command {
        Command::List { limit } => {
            for job in list_delivery_unknown_post_comment_jobs(&pool, limit).await? {
                print_job(&job);
            }
        }
        Command::Inspect { job_id } => match inspect_post_comment_job(&pool, job_id).await? {
            Some(job) => print_job(&job),
            None => anyhow::bail!("post comment job {job_id} does not exist"),
        },
        Command::MarkDelivered {
            job_id,
            bot_comment_message_id,
            actor,
            reason,
        } => {
            let result = mark_delivery_unknown_post_comment_delivered(
                &pool,
                job_id,
                bot_comment_message_id,
                OperatorAuditParams {
                    actor: &actor,
                    reason: &reason,
                },
            )
            .await?;
            print_transition(result, job_id, "sent");
        }
        Command::MarkFailed {
            job_id,
            actor,
            reason,
        } => {
            let result = mark_delivery_unknown_post_comment_failed(
                &pool,
                job_id,
                OperatorAuditParams {
                    actor: &actor,
                    reason: &reason,
                },
            )
            .await?;
            print_transition(result, job_id, "failed");
        }
        Command::Retry {
            job_id,
            actor,
            reason,
        } => {
            let Some(job) = claim_delivery_unknown_post_comment_for_operator_retry(
                &pool,
                job_id,
                OperatorAuditParams {
                    actor: &actor,
                    reason: &reason,
                },
            )
            .await?
            else {
                anyhow::bail!(
                    "post comment job {job_id} is not delivery_unknown or was claimed by another operator"
                );
            };

            // Config and Bot are deliberately initialized only after the exact operator claim.
            let config = match Config::from_env().and_then(|config| {
                config.validate_runtime_secrets()?;
                Ok(config)
            }) {
                Ok(config) => config,
                Err(error) => {
                    mark_operator_retry_post_comment_terminal_failed(
                        &pool,
                        &job,
                        tg_ai_bot_teloxide::features::first_comment::repo::CommentErrorKind::Configuration,
                    ).await?;
                    return Err(error.context("operator retry claimed the job but runtime configuration is invalid; job was terminally failed"));
                }
            };
            let token = match std::env::var("TELOXIDE_TOKEN") {
                Ok(token) if !token.trim().is_empty() => token,
                _ => {
                    mark_operator_retry_post_comment_terminal_failed(
                        &pool,
                        &job,
                        tg_ai_bot_teloxide::features::first_comment::repo::CommentErrorKind::Configuration,
                    ).await?;
                    anyhow::bail!(
                        "operator retry claimed the job but TELOXIDE_TOKEN is missing; job was terminally failed"
                    );
                }
            };
            let state = AppState::new(pool.clone(), config);
            let bot = Bot::new(token).parse_mode(ParseMode::Html);
            process_claimed_post_comment_job(&bot, &state, &job).await?;
            println!("operator retry processed post comment job {}", job.id);
        }
    }
    Ok(())
}

fn print_job(
    job: &tg_ai_bot_teloxide::features::first_comment::repo::PostCommentJobReconciliationView,
) {
    println!(
        "id={} discussion_message_id={} source_message_id={} status={} error_kind={} attempts={} operator_retry_only={} bot_comment_message_id={} created_at={} updated_at={}",
        job.id,
        job.discussion_message_id,
        job.source_message_id,
        job.status,
        job.error_kind.as_deref().unwrap_or("-"),
        job.attempts,
        job.operator_retry_only,
        job.bot_comment_message_id
            .map_or_else(|| "-".to_string(), |id| id.to_string()),
        job.created_at,
        job.updated_at,
    );
}

fn print_transition(
    result: tg_ai_bot_teloxide::features::jobs::claim::CasResult,
    job_id: i64,
    status: &str,
) {
    match result {
        tg_ai_bot_teloxide::features::jobs::claim::CasResult::Applied => {
            println!("post comment job {job_id} marked {status}")
        }
        tg_ai_bot_teloxide::features::jobs::claim::CasResult::LeaseLost => {
            println!("post comment job {job_id} was not delivery_unknown; no change made")
        }
    }
}

fn parse_args() -> anyhow::Result<Command> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        print_usage();
        anyhow::bail!("command is required");
    };
    if matches!(command.as_str(), "-h" | "--help") {
        print_usage();
        std::process::exit(0);
    }

    let mut job_id = None;
    let mut bot_comment_message_id = None;
    let mut actor = None;
    let mut reason = None;
    let mut limit = 20;
    let mut acknowledged = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--job-id" => job_id = Some(next_i64(&mut args, "--job-id")?),
            "--bot-comment-message-id" => {
                bot_comment_message_id = Some(next_i32(&mut args, "--bot-comment-message-id")?)
            }
            "--actor" => actor = Some(next_string(&mut args, "--actor")?),
            "--reason" => reason = Some(next_string(&mut args, "--reason")?),
            "--limit" => limit = next_i64(&mut args, "--limit")?,
            "--acknowledge-duplicate-risk" => acknowledged = true,
            _ => anyhow::bail!("unknown option: {arg}"),
        }
    }
    match command.as_str() {
        "list" => {
            if !(1..=MAX_LIST_LIMIT).contains(&limit) {
                anyhow::bail!("--limit must be 1..={MAX_LIST_LIMIT}");
            }
            Ok(Command::List { limit })
        }
        "inspect" => Ok(Command::Inspect {
            job_id: required(job_id, "--job-id")?,
        }),
        "mark-delivered" => Ok(Command::MarkDelivered {
            job_id: required(job_id, "--job-id")?,
            bot_comment_message_id: required(bot_comment_message_id, "--bot-comment-message-id")?,
            actor: bounded(required(actor, "--actor")?, "--actor", 128)?,
            reason: bounded(required(reason, "--reason")?, "--reason", 1000)?,
        }),
        "mark-failed" => Ok(Command::MarkFailed {
            job_id: required(job_id, "--job-id")?,
            actor: bounded(required(actor, "--actor")?, "--actor", 128)?,
            reason: bounded(required(reason, "--reason")?, "--reason", 1000)?,
        }),
        "retry" => {
            if !acknowledged {
                anyhow::bail!("retry requires --acknowledge-duplicate-risk");
            }
            Ok(Command::Retry {
                job_id: required(job_id, "--job-id")?,
                actor: bounded(required(actor, "--actor")?, "--actor", 128)?,
                reason: bounded(required(reason, "--reason")?, "--reason", 1000)?,
            })
        }
        _ => anyhow::bail!("unknown command: {command}"),
    }
}

fn next_string(args: &mut impl Iterator<Item = String>, option: &str) -> anyhow::Result<String> {
    args.next()
        .with_context(|| format!("{option} requires value"))
}
fn next_i64(args: &mut impl Iterator<Item = String>, option: &str) -> anyhow::Result<i64> {
    next_string(args, option)?
        .parse()
        .with_context(|| format!("invalid {option}"))
}
fn next_i32(args: &mut impl Iterator<Item = String>, option: &str) -> anyhow::Result<i32> {
    next_string(args, option)?
        .parse()
        .with_context(|| format!("invalid {option}"))
}
fn required<T>(value: Option<T>, option: &str) -> anyhow::Result<T> {
    value.with_context(|| format!("{option} is required"))
}
fn bounded(value: String, option: &str, max: usize) -> anyhow::Result<String> {
    if value.is_empty() || value.chars().count() > max {
        anyhow::bail!("{option} must contain 1..={max} characters");
    }
    Ok(value)
}
fn print_usage() {
    println!(
        "Usage:\n  reconcile_comment_delivery list [--limit 20]\n  reconcile_comment_delivery inspect --job-id ID\n  reconcile_comment_delivery mark-delivered --job-id ID --bot-comment-message-id ID --actor ACTOR --reason REASON\n  reconcile_comment_delivery mark-failed --job-id ID --actor ACTOR --reason REASON\n  reconcile_comment_delivery retry --job-id ID --actor ACTOR --reason REASON --acknowledge-duplicate-risk"
    );
}
