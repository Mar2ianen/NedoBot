//! Экспорт и воспроизводимый replay локального каскада. Сырые тексты — только private files.
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
};
use teloxide_antispam::{
    logreg::{LinearSpamModel, load_model},
    preprocess::{PREPROCESSING_VERSION, char_tokens, prepare_text, word_tokens},
};
use tg_ai_bot_teloxide::{
    config::Config,
    db::build_pool,
    features::new_user_audit::local_cascade::{
        CorpusExample, LocalCascadeConfig, LocalCascadeMode, PreparedCorpus, hash_bytes,
        load_corpus, model_input,
    },
};

fn private_output(path: &str) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .context("cannot create a new private output file")
}
fn read_rows(path: &str) -> anyhow::Result<Vec<CorpusExample>> {
    BufReader::new(File::open(path)?)
        .lines()
        .map(|l| Ok(serde_json::from_str(&l?)?))
        .collect()
}
fn summary(
    rows: &[CorpusExample],
    policy: &LocalCascadeConfig,
    model: Option<&LinearSpamModel>,
    temporal: bool,
    corpora: &BTreeMap<(i64, i64), PreparedCorpus>,
) -> anyhow::Result<Value> {
    let mut authors = BTreeMap::<(i64, i64), (bool, bool)>::new();
    let mut reasons = BTreeMap::<String, usize>::new();
    for row in rows {
        let e = corpora[&(row.chat_id, row.user_id)].evaluate(
            &row.text,
            row.chat_id,
            row.user_id,
            policy,
            model,
            temporal.then_some(row.message_at),
        )?;
        *reasons.entry(e.reason).or_default() += 1;
        let item = authors
            .entry((row.chat_id, row.user_id))
            .or_insert((row.label == "spam", false));
        ensure!(
            item.0 == (row.label == "spam"),
            "conflicting author labels in dataset"
        );
        item.1 |= e.candidate;
    }
    let mut spam = 0;
    let mut normal = 0;
    let mut flagged_spam = 0;
    let mut flagged_normal = 0;
    for (label, decision) in authors.values() {
        if *label {
            spam += 1;
            flagged_spam += usize::from(*decision);
        } else {
            normal += 1;
            flagged_normal += usize::from(*decision);
        }
    }
    Ok(
        json!({"authors_spam":spam,"authors_normal":normal,"review_spam":flagged_spam,
        "review_normal":flagged_normal,"potential_spam_review_coverage":if spam>0 {flagged_spam as f64/spam as f64}else{0.0},
        "reasons":reasons,"auto_enforcement_eligible":false}),
    )
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("export") if args.len() == 2 => {
            dotenvy::dotenv().ok();
            let config = Config::from_env()?;
            ensure!(
                config.discussion_chat_id < 0,
                "only group messages may enter the spam corpus"
            );
            let pool = build_pool().await?;
            let rows = load_corpus(&pool, config.discussion_chat_id, 50_000).await?;
            ensure!(rows.len() <= 50_000, "corpus exceeds export limit");
            let mut output = private_output(&args[1])?;
            for row in &rows {
                writeln!(output, "{}", serde_json::to_string(row)?)?;
            }
            println!(
                "{}",
                json!({"instance":config.community.instance.id,"examples":rows.len(),"label_unit":"author_review"})
            );
        }
        Some("replay") if args.len() == 4 => {
            let rows = read_rows(&args[1])?;
            let mut corpora = BTreeMap::new();
            for row in &rows {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    corpora.entry((row.chat_id, row.user_id))
                {
                    let holdout = rows
                        .iter()
                        .filter(|r| (r.chat_id, r.user_id) != (row.chat_id, row.user_id))
                        .cloned()
                        .collect::<Vec<_>>();
                    entry.insert(PreparedCorpus::new(&holdout)?);
                }
            }
            let model = if args[2] == "-" {
                None
            } else {
                Some(load_model(&std::fs::read_to_string(&args[2])?)?)
            };
            let policy = LocalCascadeConfig {
                mode: LocalCascadeMode::ReviewOnly,
                ..Default::default()
            };
            let mut grid = Vec::new();
            // 30 вариантов без добавления новых lexical markers.
            for template in [0.75, 0.85, 0.95] {
                for authors in [1, 2] {
                    for linear in [0.90, 0.95, 0.98, 0.99, 0.999] {
                        let p = LocalCascadeConfig {
                            template_similarity: template,
                            min_template_authors: authors,
                            linear_review_threshold: linear,
                            ..policy.clone()
                        };
                        grid.push(json!({"policy":p,"leave_one_author_out":summary(&rows,&p,model.as_ref(),false,&corpora)?}));
                    }
                }
            }
            let report = json!({"version":"spam-cascade-replay-v1","label_unit":"author_review",
                "dataset_sha256":hash_bytes(&std::fs::read(&args[1])?),"messages":rows.len(),
                "model_sha256":if args[2]=="-" {None}else{Some(hash_bytes(&std::fs::read(&args[2])?))},
                "model_version":model.as_ref().map(|m| &m.version),"policy":policy,
                "leave_one_author_out":summary(&rows,&policy,model.as_ref(),false,&corpora)?,
                "strict_message_time":summary(&rows,&policy,model.as_ref(),true,&corpora)?,"grid":grid,
                "limitations":["Author labels are not independent message-level gold labels.",
                    "The existing model may have seen historical authors during training.",
                    "Current-label exports omit revoked past labels; temporal replay is not a complete historical reconstruction.",
                    "Review coverage is not end-to-end recall or measured LLM savings."]});
            serde_json::to_writer_pretty(private_output(&args[3])?, &report)?;
            println!(
                "{}",
                json!({"report":args[3],"messages":rows.len(),"auto_enforcement_eligible":false})
            );
        }
        Some("verify") if args.len() == 4 => {
            let rows = read_rows(&args[1])?;
            let model = load_model(&std::fs::read_to_string(&args[2])?)?;
            let expected: Vec<f64> = serde_json::from_str(&std::fs::read_to_string(&args[3])?)?;
            ensure!(
                !rows.is_empty() && rows.len() == expected.len(),
                "parity row count mismatch"
            );
            let mut largest = 0.0_f64;
            for (row, expected) in rows.iter().zip(expected) {
                let actual =
                    teloxide_antispam::logreg::spam_probability(&model, &model_input(&row.text));
                ensure!(
                    actual.is_finite() && expected.is_finite(),
                    "nonfinite parity result"
                );
                largest = largest.max((actual - expected).abs());
            }
            ensure!(largest < 1e-10, "Python/Rust inference mismatch");
            println!(
                "{}",
                json!({"verified_messages":rows.len(),"maximum_probability_difference":largest})
            );
        }
        Some("tokens") if args.len() == 3 => {
            let mut output = private_output(&args[2])?;
            for row in read_rows(&args[1])? {
                let text = prepare_text(&model_input(&row.text)).model_text();
                let tokens = teloxide_antispam::text::token_set(&text);
                let mut sorted = tokens.into_iter().collect::<Vec<_>>();
                sorted.sort();
                let group = hash_bytes(sorted.join(" ").as_bytes());
                writeln!(
                    output,
                    "{}",
                    json!({"example":row,"preprocessing":PREPROCESSING_VERSION,
                    "exact_campaign":group,"word":word_tokens(&text),"character":char_tokens(&text)})
                )?;
            }
        }
        _ => anyhow::bail!(
            "usage: spam_cascade export <private.jsonl> | replay <dataset.jsonl> <model.json|-> <report.json> | tokens <dataset.jsonl> <private-tokens.jsonl> | verify <dataset.jsonl> <model.json> <parity.json>"
        ),
    }
    Ok(())
}
