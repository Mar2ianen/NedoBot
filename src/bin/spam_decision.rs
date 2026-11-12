//! Закрытый диагностический CLI. Сырые тексты и вероятности — только private output.
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use std::{fs::OpenOptions, io::Write};
use tg_ai_bot_teloxide::{
    config::Config,
    db::build_pool,
    features::{
        decision_audit::{
            client::{DecisionClient, POLICY_VERSION, QUESTIONS},
            service::configuration_hash,
        },
        new_user_audit::{
            diagnostics::{MissedStage, analyze_text},
            local_cascade::hash_bytes,
        },
    },
};

fn output(path: &str, value: &Value) -> anyhow::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .context("cannot create new private output")?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        args.len() >= 3,
        "usage: spam_decision probe OUTPUT | trace OUTPUT CHAT MESSAGE | replay INPUT OUTPUT"
    );
    let mut config = Config::from_env()?;
    match args[0].as_str() {
        "probe" => {
            // Только синтетические fixtures; явный запуск не меняет runtime config/очередь.
            ensure!(
                args.len() == 3 && args[1] == "--synthetic",
                "usage: spam_decision probe --synthetic OUTPUT"
            );
            config.community.moderation.decision.enabled = true;
            config
                .community
                .moderation
                .decision
                .external_processing_allowed = true;
            let client = DecisionClient::new(&config.community.moderation.decision)?;
            let corpus: Value =
                serde_json::from_str(include_str!("../../tests/fixtures/antispam_v2.json"))?;
            let mut results = Vec::new();
            for case in corpus["cases"]
                .as_array()
                .context("fixture cases missing")?
            {
                let input = json!({"author_message":case["text"],"chat_policy":config.community.moderation.decision.chat_policy});
                match client.assess(&input,None).await {
                    Ok((assessment,latency))=>results.push(json!({"id":case["id"],"input_hash":hash_bytes(&serde_json::to_vec(&input)?),"assessment":assessment,"latency_ms":latency,"verdict":assessment.verdict()})),
                    Err(e)=>results.push(json!({"id":case["id"],"error_kind":e.kind()})),
                }
            }
            let successes = results
                .iter()
                .filter(|r| r.get("assessment").is_some())
                .count();
            output(
                &args[2],
                &json!({"synthetic_only":true,"policy":POLICY_VERSION,"questions":hash_bytes(QUESTIONS.as_bytes()),"model":config.community.moderation.decision.model,"results":results}),
            )?;
            println!(
                "synthetic decisions: {successes}/{} validated",
                results.len()
            );
            ensure!(
                successes == results.len(),
                "some synthetic decisions were unavailable or invalid"
            );
        }
        "trace" => {
            ensure!(
                args.len() == 4,
                "usage: spam_decision trace OUTPUT CHAT MESSAGE"
            );
            let chat: i64 = args[2].parse()?;
            let message: i32 = args[3].parse()?;
            ensure!(
                config.chat_by_id(chat).is_some(),
                "chat outside configured community"
            );
            let pool = build_pool().await?;
            let mut tx = pool.begin().await?;
            sqlx::query("set transaction read only")
                .execute(&mut *tx)
                .await?;
            let row:Option<Value>=sqlx::query_scalar(r#"select jsonb_build_object(
              'text',m.text,'created_at',m.created_at,'edited_at',m.edited_at,'deleted_by_bot_at',m.deleted_by_bot_at,
              'message_count',u.message_count,'age_seconds',extract(epoch from now()-u.first_seen_at)::bigint,'member_status',u.member_status,
              'risk_score',a.risk_score,'risk_signals',jsonb_build_object('baseline',a.risk_baseline_signals,'first_message',a.risk_first_message_signals,'avatar',a.risk_avatar_signals,'external',a.risk_external_signals),
              'audit',(select to_jsonb(j)-'input_json' from new_user_audit_jobs j where j.chat_id=m.chat_id and j.telegram_user_id=m.user_id and j.created_at>=m.created_at order by created_at,id limit 1),
              'captcha',(select jsonb_build_object('status',c.status,'setup_error_kind',c.setup_error_kind,'created_at',c.created_at,'updated_at',c.updated_at) from telegram_risk_captcha_challenges c where c.chat_id=m.chat_id and c.telegram_user_id=m.user_id),
              'observations',(select to_jsonb(o) from moderation_text_observations o where o.instance_id=$3 and o.chat_id=m.chat_id and o.message_id=m.message_id),
              'decisions',(select jsonb_agg(to_jsonb(d)-'avatar_file_id') from moderation_decision_jobs d where d.instance_id=$3 and d.chat_id=m.chat_id and d.message_id=m.message_id))
              from telegram_messages m left join telegram_chat_users u on u.chat_id=m.chat_id and u.telegram_user_id=m.user_id
              left join telegram_new_user_profile_audits a on a.chat_id=m.chat_id and a.telegram_user_id=m.user_id where m.chat_id=$1 and m.message_id=$2"#)
              .bind(chat).bind(message).bind(&config.community.instance.id).fetch_optional(&mut *tx).await?;
            tx.commit().await?;
            let trace = if let Some(row) = row {
                let text = row["text"].as_str().unwrap_or_default();
                let local = analyze_text(
                    text,
                    row["message_count"].as_i64().unwrap_or(0),
                    row["age_seconds"].as_i64(),
                );
                let a = &row["audit"];
                let first = &a["assessment_json"]["first_message_assessment"];
                let recognized =
                    first["offtopic_promo"] == true || first["direct_dm_offer"] == true;
                let stage = if !row["deleted_by_bot_at"].is_null() {
                    MissedStage::NoConfirmedMiss
                } else if a["status"] == "failed" || a["materialization_status"] == "failed" {
                    MissedStage::Failed
                } else if recognized && row["captcha"]["setup_error_kind"].as_str().is_some() {
                    MissedStage::Delivery
                } else if recognized {
                    MissedStage::Policy
                } else if row["message_count"].as_i64().is_some_and(|n| n > 10)
                    || row["age_seconds"].as_i64().is_some_and(|s| s > 259_200)
                {
                    MissedStage::NotEligible
                } else if a.is_null() {
                    MissedStage::Features
                } else {
                    MissedStage::Classification
                };
                json!({"instance":config.community.instance.id,"message_id":message,"stage":stage,
                    "historical_gate_note":"Текущее состояние не доказывает историческую причину policy gate; сверять журнал по job ID. Отсутствие санкции не доказывает ошибку классификации.",
                    "current_policy":{"enforce":config.community.moderation.enforce_enabled,"review_delivery":config.community.moderation.review_delivery_enabled,"captcha":config.community.moderation.captcha_enabled},"local_trace":local,"persisted":row})
            } else {
                json!({"stage":MissedStage::NotIngested,"message_id":message})
            };
            output(&args[1], &trace)?;
            println!("trace stage: {}", trace["stage"]);
        }
        "enqueue-recent" => {
            ensure!(
                args.len() == 3 && args[1] == "--limit",
                "usage: spam_decision enqueue-recent --limit N"
            );
            let limit: i64 = args[2].parse()?;
            ensure!((1..=20).contains(&limit), "limit must be 1..20");
            config.validate_runtime_secrets()?;
            ensure!(
                config.community.moderation.decision.enabled,
                "decision disabled"
            );
            let chats: Vec<i64> = config
                .community
                .chats
                .values()
                .filter(|c| c.ingest && c.moderation)
                .map(|c| c.id)
                .collect();
            let pool = build_pool().await?;
            let (max_messages, max_age) =
                tg_ai_bot_teloxide::features::new_user_audit::diagnostics::newcomer_limits();
            let messages:Vec<(i64,i32)>=sqlx::query_as("select distinct on (m.chat_id,m.user_id) m.chat_id,m.message_id from telegram_messages m join telegram_chat_users u on u.chat_id=m.chat_id and u.telegram_user_id=m.user_id where m.chat_id=any($1) and m.created_at>now()-interval '24 hours' and m.source_channel_id is null and m.sender_chat_id is null and nullif(trim(m.text),'') is not null and u.message_count between 1 and $3 and u.first_seen_at>now()-make_interval(secs=>$4::double precision) order by m.chat_id,m.user_id,m.created_at desc,m.message_id desc limit $2")
                .bind(chats).bind(limit).bind(max_messages).bind(max_age as f64).fetch_all(&pool).await?;
            for (chat, message) in &messages {
                tg_ai_bot_teloxide::features::decision_audit::service::observe_message(
                    &pool, &config, *chat, *message,
                )
                .await?;
            }
            println!(
                "observed {} recent author messages; only shadow queue, no sanctions",
                messages.len()
            );
        }
        "compare" => {
            ensure!(
                args.len() == 3,
                "usage: spam_decision compare FROZEN_JSONL OUTPUT"
            );
            let rows: Vec<tg_ai_bot_teloxide::features::decision_audit::evaluation::FrozenExample> =
                std::fs::read_to_string(&args[1])?
                    .lines()
                    .map(serde_json::from_str)
                    .collect::<Result<_, _>>()?;
            let report = tg_ai_bot_teloxide::features::decision_audit::evaluation::compare(
                &rows,
                &config.community.instance.id,
                &config.community.moderation.decision.model,
            )?;
            output(&args[2], &report)?;
            println!(
                "compared {} frozen messages; no API calls or actions",
                rows.len()
            );
        }
        "replay" => {
            ensure!(args.len() == 3, "usage: spam_decision replay INPUT OUTPUT");
            let rows: Vec<Value> = std::fs::read_to_string(&args[1])?
                .lines()
                .map(serde_json::from_str)
                .collect::<Result<_, _>>()?;
            let mut out = Vec::new();
            for row in &rows {
                // Старые author labels не превращаются в message-level ground truth.
                let text = row["text"].as_str().context("text missing")?;
                out.push(json!({"chat_id":row["chat_id"],"user_id":row["user_id"],"message_id":row["message_id"],
                    "source_hash":hash_bytes(text.as_bytes()),"label":row["label"],"label_level":row.get("label_level").cloned().unwrap_or(json!("author_weak")),
                    "trace":analyze_text(text,1,Some(0))}));
            }
            output(
                &args[2],
                &json!({"configuration_hash":configuration_hash(&config),"rows":out,"note":"Replay измеряет lexical A/B при synthetic count=1/age=0 и наблюдения, не production eligibility или end-to-end sanctions. C/D требуют замороженных Decision assessments; ошибки API не ham."}),
            )?;
            println!(
                "replayed {} frozen rows; no API calls or actions",
                rows.len()
            );
        }
        _ => anyhow::bail!("unknown command"),
    }
    Ok(())
}
