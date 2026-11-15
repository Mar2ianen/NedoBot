use std::{
    collections::BTreeMap,
    env,
    fs::{self, File, OpenOptions},
    io::{BufReader, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::Context;
use tg_ai_bot_teloxide::private_corpus::{
    InstanceSummary, PrivateUrlRecord, extract_private_records, summarize_instances,
};

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    anyhow::ensure!(
        env::var("NEDOBOT_PRIVATE_CORPUS_APPROVED").as_deref() == Ok("YES"),
        "set NEDOBOT_PRIVATE_CORPUS_APPROVED=YES after explicit authorization"
    );
    ensure_private_directory(&args.output_dir)?;
    for input in [
        &args.nedonews_input,
        &args.nedonews_qr,
        &args.pvo_input,
        &args.pvo_qr,
    ] {
        ensure_private_file(input)?;
    }

    let (nedonews, nedonews_summary) =
        extract_file_set("nedonews", &args.nedonews_input, &args.nedonews_qr)?;
    let (pvo, pvo_summary) = extract_file_set("pvo", &args.pvo_input, &args.pvo_qr)?;
    let instances = BTreeMap::from([
        ("nedonews".to_owned(), nedonews_summary),
        ("pvo".to_owned(), pvo_summary),
    ]);
    let records = BTreeMap::from([
        ("nedonews".to_owned(), nedonews.clone()),
        ("pvo".to_owned(), pvo.clone()),
    ]);
    let summary = summarize_instances(instances, &records)?;

    write_jsonl_new(&args.output_dir.join("nedonews.manifest.jsonl"), &nedonews)?;
    write_jsonl_new(&args.output_dir.join("pvo.manifest.jsonl"), &pvo)?;
    write_json_new(&args.output_dir.join("summary.json"), &summary)?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}

struct Args {
    output_dir: PathBuf,
    nedonews_input: PathBuf,
    nedonews_qr: PathBuf,
    pvo_input: PathBuf,
    pvo_qr: PathBuf,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut values = BTreeMap::new();
    let mut args = env::args().skip(1);
    while let Some(key) = args.next() {
        if key == "--authorized-read-only" {
            values.insert(key, "true".to_owned());
            continue;
        }
        anyhow::ensure!(key.starts_with("--"), "unexpected argument");
        let value = args.next().context("option requires a value")?;
        anyhow::ensure!(!value.starts_with("--"), "option requires a value");
        anyhow::ensure!(values.insert(key, value).is_none(), "duplicate option");
    }
    anyhow::ensure!(
        values.remove("--authorized-read-only").is_some(),
        "--authorized-read-only is required"
    );
    let take = |values: &mut BTreeMap<String, String>, key: &str| {
        values
            .remove(key)
            .map(PathBuf::from)
            .with_context(|| format!("missing required option {key}"))
    };
    let output_dir = take(&mut values, "--output-dir")?;
    let nedonews_input = take(&mut values, "--nedonews-input")?;
    let nedonews_qr = take(&mut values, "--nedonews-qr")?;
    let pvo_input = take(&mut values, "--pvo-input")?;
    let pvo_qr = take(&mut values, "--pvo-qr")?;
    anyhow::ensure!(values.is_empty(), "unknown option");
    Ok(Args {
        output_dir,
        nedonews_input,
        nedonews_qr,
        pvo_input,
        pvo_qr,
    })
}

fn extract_file_set(
    instance: &str,
    message_path: &Path,
    qr_path: &Path,
) -> anyhow::Result<(Vec<PrivateUrlRecord>, InstanceSummary)> {
    let messages = File::open(message_path)
        .with_context(|| format!("open private {instance} message export"))?;
    let qr = File::open(qr_path).with_context(|| format!("open private {instance} QR export"))?;
    extract_private_records(instance, BufReader::new(messages), BufReader::new(qr))
}

fn ensure_private_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::metadata(path).context("private output directory must already exist")?;
    anyhow::ensure!(metadata.is_dir(), "private output path is not a directory");
    anyhow::ensure!(
        metadata.permissions().mode() & 0o777 == 0o700,
        "private output directory must have mode 0700"
    );
    Ok(())
}

fn ensure_private_file(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).context("private source file is missing")?;
    anyhow::ensure!(
        metadata.is_file(),
        "private source path is not a regular file"
    );
    anyhow::ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "private source file must not grant group/other access"
    );
    anyhow::ensure!(
        !metadata.file_type().is_symlink(),
        "private source file must not be a symlink"
    );
    Ok(())
}

fn private_file(path: &Path) -> anyhow::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create private output {}", path.display()))
}

fn write_jsonl_new<T: serde::Serialize>(path: &Path, records: &[T]) -> anyhow::Result<()> {
    let mut file = private_file(path)?;
    for record in records {
        serde_json::to_writer(&mut file, record)?;
        file.write_all(b"\n")?;
    }
    file.sync_all()?;
    Ok(())
}

fn write_json_new<T: serde::Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let mut file = private_file(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
