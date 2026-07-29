use tg_ai_bot_teloxide::{
    db::build_pool,
    features::jobs::observability::{JobErrorMetrics, JobQueueMetrics, load_job_lifecycle_report},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    ensure_no_arguments()?;
    let pool = build_pool().await?;
    let report = load_job_lifecycle_report(&pool).await?;
    for queue in &report {
        print_queue(queue);
    }
    Ok(())
}

fn ensure_no_arguments() -> anyhow::Result<()> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        None => Ok(()),
        Some("-h" | "--help") => {
            println!(
                "Usage: job_lifecycle_report\n\nRead-only lifecycle report; requires only DATABASE_URL."
            );
            std::process::exit(0);
        }
        Some(argument) => anyhow::bail!("unknown option: {argument}"),
    }
}

fn print_queue(queue: &JobQueueMetrics) {
    println!("{}:", queue.queue);
    println!(
        "  oldest_ready_age_seconds={}",
        format_age(queue.oldest_ready_age_seconds)
    );
    println!("  lease_reclaim_count={}", queue.lease_reclaim_count);
    for status in &queue.statuses {
        println!(
            "  status={} jobs={} attempts={}",
            status.status, status.jobs, status.attempts
        );
    }
    for error in &queue.errors {
        print_error(error);
    }
    if let Some(failures) = queue.embedding_batch_cardinality_failures {
        println!("  embedding_batch_cardinality_failures={failures}");
    }
}

fn print_error(error: &JobErrorMetrics) {
    println!(
        "  error_kind={} jobs={} attempts={} terminal_failures={}",
        error.error_kind, error.jobs, error.attempts, error.terminal_failures
    );
}

fn format_age(age: Option<f64>) -> String {
    age.map(|seconds| format!("{seconds:.0}"))
        .unwrap_or_else(|| "-".to_string())
}
