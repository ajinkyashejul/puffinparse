//! Live tests of the asynchronous jobs API (`submit_parse` / `retrieve_parse`). Ignored by default;
//! each makes one billed 1-page parse. Run with
//! `cargo test -p puffinparse-core --test live_jobs -- --ignored --nocapture` when the keys are set.

use puffinparse_core::{retrieve_parse, submit_parse, DocumentRequest, JobHandle, JobStatus};
use std::time::{Duration, Instant};

const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/invoice_001.png");

async fn submit_and_retrieve(model: &str, env_var: &str) {
    if std::env::var(env_var).map(|v| v.trim().is_empty()).unwrap_or(true) {
        eprintln!("skipping {model}: {env_var} not set");
        return;
    }
    let handle = submit_parse(DocumentRequest::from_path(SAMPLE).model(model)).await.expect("submit succeeds");
    eprintln!("{model}: submitted job {}", handle.job_id);
    assert_eq!(handle.model, model);

    // The handle is meant to cross process boundaries: round-trip it through JSON first.
    let handle: JobHandle = serde_json::from_str(&serde_json::to_string(&handle).unwrap()).unwrap();
    let started = Instant::now();
    let mut polls = 0;
    let resp = loop {
        polls += 1;
        match retrieve_parse(&handle).await.expect("status check succeeds") {
            JobStatus::Pending => {
                assert!(started.elapsed() < Duration::from_secs(300), "job did not finish in 5 minutes");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            JobStatus::Succeeded(resp) => break resp,
            JobStatus::Failed(e) => panic!("job failed: {e}"),
        }
    };
    eprintln!(
        "{model}: done after {polls} polls, {} ms, pages={} cost={:?}",
        resp.latency_ms, resp.usage.pages, resp.cost_usd
    );
    assert_eq!(resp.model, model);
    assert_eq!(resp.provider_job_id.as_deref(), Some(handle.job_id.as_str()));
    assert_eq!(resp.usage.pages, 1);
    assert!(resp.markdown.contains("Cedar Ridge Supply"), "{}", resp.markdown);
    assert!(resp.cost_usd.unwrap_or(0.0) > 0.0);
}

#[tokio::test]
#[ignore = "needs REDUCTO_API_KEY and network (one billed page)"]
async fn reducto_submit_retrieve_live() {
    submit_and_retrieve("reducto/standard", "REDUCTO_API_KEY").await;
}

#[tokio::test]
#[ignore = "needs EXTEND_API_KEY and network (one billed page)"]
async fn extend_submit_retrieve_live() {
    submit_and_retrieve("extend/parse_light", "EXTEND_API_KEY").await;
}

#[tokio::test]
#[ignore = "needs LLAMA_API_KEY and network (one billed page)"]
async fn llamaparse_submit_retrieve_live() {
    submit_and_retrieve("llamaparse/fast", "LLAMA_API_KEY").await;
}
