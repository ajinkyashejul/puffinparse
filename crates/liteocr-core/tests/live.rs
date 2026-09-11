//! Live provider tests. Ignored by default; run with
//! `cargo test -p liteocr-core --test live -- --ignored` when the provider keys are set.

use liteocr_core::{ocr, OcrRequest};

const SAMPLE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/multipage_001.pdf");

async fn check(model: &str, env_var: &str) {
    if std::env::var(env_var).map(|v| v.is_empty()).unwrap_or(true) {
        eprintln!("skipping {model}: {env_var} not set");
        return;
    }
    let resp = ocr(OcrRequest::from_path(SAMPLE).model(model).timeout_secs(240.0)).await.expect("ocr succeeds");
    assert_eq!(resp.model, model);
    assert_eq!(resp.usage.pages, 2);
    assert_eq!(resp.pages.len(), 2);
    assert!(!resp.markdown.trim().is_empty());
    assert!(resp.cost_usd.unwrap_or(0.0) > 0.0);
    assert!(resp.pages.iter().any(|p| !p.blocks.is_empty()));
}

#[tokio::test]
#[ignore = "needs REDUCTO_API_KEY and network"]
async fn reducto_live() {
    check("reducto/standard", "REDUCTO_API_KEY").await;
}

#[tokio::test]
#[ignore = "needs EXTEND_API_KEY and network"]
async fn extend_live() {
    check("extend/parse_light", "EXTEND_API_KEY").await;
}

#[tokio::test]
#[ignore = "needs LLAMA_API_KEY and network"]
async fn llamaparse_live() {
    check("llamaparse/fast", "LLAMA_API_KEY").await;
}
