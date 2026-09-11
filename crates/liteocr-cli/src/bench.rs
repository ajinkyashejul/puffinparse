//! `liteocr bench` — run the open benchmark and render reports.

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use futures::stream::{self, StreamExt};
use liteocr_core::bench::{score, summarize, Metrics, NormalizeOptions, Summary};
use liteocr_core::{OcrRequest, OutputFormat};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Subcommand, Debug)]
pub enum BenchCommand {
    /// Run models over a dataset and write a result JSON.
    Run(RunArgs),
    /// Render one or more result JSON files as a leaderboard.
    Report(ReportArgs),
    /// Score a single prediction file against a truth file (no network).
    Score { prediction: PathBuf, truth: PathBuf },
}

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    /// Dataset directory containing manifest.json.
    #[arg(short, long)]
    dataset: PathBuf,
    /// Models to evaluate ("<provider>/<model>"). Repeatable.
    #[arg(short, long, required = true, num_args = 1..)]
    models: Vec<String>,
    /// Output JSON path (default: benchmark/results/<date>-<dataset>.json).
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Concurrent requests per model.
    #[arg(short, long, default_value_t = 4)]
    concurrency: usize,
    /// Only run documents whose id contains this substring.
    #[arg(long)]
    filter: Option<String>,
    /// Limit the number of documents.
    #[arg(long)]
    limit: Option<usize>,
    /// Per-call timeout in seconds.
    #[arg(long, default_value_t = 300.0)]
    timeout: f64,
    /// Directory to save each model's raw markdown output per document (for inspection).
    #[arg(long)]
    save_outputs: Option<PathBuf>,
    /// Score case-sensitively.
    #[arg(long)]
    case_sensitive: bool,
    /// Allow provider-side result caches (LlamaParse re-parses within 48 h are cached and near-instant).
    /// Off by default so latency reflects real work.
    #[arg(long)]
    allow_cache: bool,
}

#[derive(clap::Args, Debug)]
pub struct ReportArgs {
    /// Result JSON files.
    #[arg(required = true, num_args = 1..)]
    results: Vec<PathBuf>,
    /// Output format.
    #[arg(short, long, default_value = "markdown")]
    format: String,
}

// ---- dataset manifest ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub license: String,
    pub documents: Vec<ManifestDoc>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ManifestDoc {
    pub id: String,
    pub file: String,
    pub truth: String,
    #[serde(default = "one")]
    pub pages: u32,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn one() -> u32 {
    1
}

// ---- result file ---------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RunResult {
    pub run_id: String,
    pub created_at: String,
    pub liteocr_version: String,
    pub dataset: DatasetInfo,
    pub normalize: NormalizeOptions,
    pub models: Vec<ModelResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DatasetInfo {
    pub name: String,
    pub version: String,
    pub documents: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelResult {
    pub model: String,
    pub docs: Vec<DocResult>,
    pub summary: ModelSummary,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DocResult {
    pub id: String,
    pub category: String,
    pub pages: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelSummary {
    #[serde(flatten)]
    pub accuracy: Summary,
    pub latency_p50_ms: u64,
    pub latency_p95_ms: u64,
    pub latency_per_page_ms: f64,
    pub total_pages: u32,
    pub total_cost_usd: f64,
    pub cost_per_1k_pages_usd: Option<f64>,
    pub by_category: BTreeMap<String, Summary>,
}

pub async fn run(cmd: BenchCommand) -> Result<()> {
    match cmd {
        BenchCommand::Run(args) => run_bench(args).await,
        BenchCommand::Report(args) => report(args),
        BenchCommand::Score { prediction, truth } => {
            let p =
                std::fs::read_to_string(&prediction).with_context(|| format!("reading {}", prediction.display()))?;
            let t = std::fs::read_to_string(&truth).with_context(|| format!("reading {}", truth.display()))?;
            println!("{}", serde_json::to_string_pretty(&score(&p, &t, NormalizeOptions::default()))?);
            Ok(())
        }
    }
}

fn load_manifest(dir: &Path) -> Result<(Manifest, String)> {
    let path = dir.join("manifest.json");
    let raw = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: Manifest = serde_json::from_slice(&raw).with_context(|| format!("parsing {}", path.display()))?;
    // Hash manifest + truth files so results are tied to an exact dataset revision.
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(&raw);
    for d in &manifest.documents {
        if let Ok(t) = std::fs::read(dir.join(&d.truth)) {
            h.update(&t);
        }
        if let Ok(f) = std::fs::read(dir.join(&d.file)) {
            h.update(&f);
        }
    }
    Ok((manifest, format!("{:x}", h.finalize())))
}

async fn run_bench(args: RunArgs) -> Result<()> {
    let (manifest, sha) = load_manifest(&args.dataset)?;
    let mut docs: Vec<ManifestDoc> = manifest
        .documents
        .iter()
        .filter(|d| args.filter.as_ref().map(|f| d.id.contains(f)).unwrap_or(true))
        .cloned()
        .collect();
    if let Some(n) = args.limit {
        docs.truncate(n);
    }
    if docs.is_empty() {
        bail!("no documents selected");
    }
    for m in &args.models {
        liteocr_core::ModelRef::parse(m)?;
    }
    let norm = NormalizeOptions { case_insensitive: !args.case_sensitive, ..NormalizeOptions::default() };
    eprintln!("dataset {} v{} ({} docs, sha256 {}…)", manifest.name, manifest.version, docs.len(), &sha[..12]);

    let mut models = Vec::new();
    for model in &args.models {
        eprintln!("\n== {model} ==");
        let bar = indicatif::ProgressBar::new(docs.len() as u64);
        bar.set_style(
            indicatif::ProgressStyle::with_template("{msg} [{bar:30}] {pos}/{len} ({elapsed})")
                .unwrap()
                .progress_chars("=> "),
        );
        bar.set_message(model.clone());
        let results: Vec<DocResult> = stream::iter(docs.iter().cloned())
            .map(|doc| {
                let dataset = args.dataset.clone();
                let model = model.clone();
                let bar = bar.clone();
                let save = args.save_outputs.clone();
                let timeout = args.timeout;
                let allow_cache = args.allow_cache;
                async move {
                    let r = run_doc(&dataset, &doc, &model, timeout, norm, save.as_deref(), allow_cache).await;
                    bar.inc(1);
                    r
                }
            })
            .buffer_unordered(args.concurrency.max(1))
            .collect()
            .await;
        bar.finish_and_clear();
        let mut results = results;
        results.sort_by(|a, b| a.id.cmp(&b.id));
        let summary = summarize_model(&results);
        eprintln!(
            "overall {:.2}  cer {:.3}  wer {:.3}  p50 {} ms  ${:.4} total  failed {}",
            summary.accuracy.overall,
            summary.accuracy.cer,
            summary.accuracy.wer,
            summary.latency_p50_ms,
            summary.total_cost_usd,
            summary.accuracy.failed
        );
        for d in results.iter().filter(|d| d.error.is_some()) {
            eprintln!("  ✗ {}: {}", d.id, d.error.as_deref().unwrap_or(""));
        }
        models.push(ModelResult { model: model.clone(), docs: results, summary });
    }

    let now = chrono::Utc::now();
    let result = RunResult {
        run_id: uuid_like(&now),
        created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        liteocr_version: liteocr_core::VERSION.to_string(),
        dataset: DatasetInfo {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            documents: docs.len(),
            sha256: sha,
        },
        normalize: norm,
        models,
    };
    let out = args.out.unwrap_or_else(|| {
        PathBuf::from(format!("benchmark/results/{}-{}.json", now.format("%Y-%m-%d"), manifest.name))
    });
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, serde_json::to_string_pretty(&result)?)?;
    eprintln!("\nwrote {}", out.display());
    println!("{}", render_markdown(&[result]));
    Ok(())
}

fn uuid_like(now: &chrono::DateTime<chrono::Utc>) -> String {
    format!("run-{}", now.format("%Y%m%dT%H%M%SZ"))
}

/// Provider options that defeat server-side result caches, so measured latency is real work.
fn cache_busting_options(model: &str) -> Option<serde_json::Value> {
    match model.split('/').next() {
        Some("llamaparse") => Some(serde_json::json!({ "do_not_cache": true, "invalidate_cache": true })),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_doc(
    dataset: &Path,
    doc: &ManifestDoc,
    model: &str,
    timeout: f64,
    norm: NormalizeOptions,
    save: Option<&Path>,
    allow_cache: bool,
) -> DocResult {
    let truth = match std::fs::read_to_string(dataset.join(&doc.truth)) {
        Ok(t) => t,
        Err(e) => {
            return DocResult {
                id: doc.id.clone(),
                category: doc.category.clone(),
                pages: doc.pages,
                metrics: None,
                latency_ms: 0,
                cost_usd: None,
                error: Some(format!("truth unreadable: {e}")),
            }
        }
    };
    let mut req = OcrRequest::from_path(dataset.join(&doc.file))
        .model(model)
        .timeout_secs(timeout)
        .output(OutputFormat::Markdown);
    if !allow_cache {
        if let Some(opts) = cache_busting_options(model) {
            req = req.provider_options(opts);
        }
    }
    match liteocr_core::ocr(req).await {
        Ok(resp) => {
            if let Some(dir) = save {
                let d = dir.join(model.replace('/', "_"));
                let _ = std::fs::create_dir_all(&d);
                let _ = std::fs::write(d.join(format!("{}.md", doc.id)), &resp.markdown);
            }
            DocResult {
                id: doc.id.clone(),
                category: doc.category.clone(),
                pages: resp.usage.pages,
                metrics: Some(score(&resp.markdown, &truth, norm)),
                latency_ms: resp.latency_ms,
                cost_usd: resp.cost_usd,
                error: None,
            }
        }
        Err(e) => DocResult {
            id: doc.id.clone(),
            category: doc.category.clone(),
            pages: doc.pages,
            metrics: None,
            latency_ms: 0,
            cost_usd: None,
            error: Some(e.to_string()),
        },
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn summarize_model(docs: &[DocResult]) -> ModelSummary {
    let accuracy = summarize(&docs.iter().map(|d| d.metrics).collect::<Vec<_>>());
    let mut lat: Vec<u64> = docs.iter().filter(|d| d.error.is_none()).map(|d| d.latency_ms).collect();
    lat.sort_unstable();
    let total_pages: u32 = docs.iter().filter(|d| d.error.is_none()).map(|d| d.pages).sum();
    let total_latency: u64 = lat.iter().sum();
    let total_cost: f64 = docs.iter().filter_map(|d| d.cost_usd).sum();
    let mut by_cat: BTreeMap<String, Vec<Option<Metrics>>> = BTreeMap::new();
    for d in docs {
        by_cat.entry(d.category.clone()).or_default().push(d.metrics);
    }
    ModelSummary {
        accuracy,
        latency_p50_ms: percentile(&lat, 0.5),
        latency_p95_ms: percentile(&lat, 0.95),
        latency_per_page_ms: if total_pages == 0 { 0.0 } else { total_latency as f64 / f64::from(total_pages) },
        total_pages,
        total_cost_usd: total_cost,
        cost_per_1k_pages_usd: if total_pages == 0 { None } else { Some(total_cost / f64::from(total_pages) * 1000.0) },
        by_category: by_cat.into_iter().map(|(k, v)| (k, summarize(&v))).collect(),
    }
}

fn report(args: ReportArgs) -> Result<()> {
    let mut runs = Vec::new();
    for p in &args.results {
        let raw = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
        runs.push(serde_json::from_str::<RunResult>(&raw).with_context(|| format!("parsing {}", p.display()))?);
    }
    match args.format.as_str() {
        "markdown" | "md" => println!("{}", render_markdown(&runs)),
        "json" => println!("{}", serde_json::to_string_pretty(&runs.iter().flat_map(|r| r.models.iter().map(|m| serde_json::json!({"run_id": r.run_id, "dataset": r.dataset.name, "model": m.model, "summary": m.summary}))).collect::<Vec<_>>())?),
        other => bail!("unknown format '{other}' (markdown|json)"),
    }
    Ok(())
}

fn render_markdown(runs: &[RunResult]) -> String {
    let mut rows: Vec<(&RunResult, &ModelResult)> =
        runs.iter().flat_map(|r| r.models.iter().map(move |m| (r, m))).collect();
    rows.sort_by(|a, b| {
        b.1.summary.accuracy.overall.partial_cmp(&a.1.summary.accuracy.overall).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = String::new();
    out.push_str("| Rank | Model | Overall | Char sim | CER | WER | Word F1 | Order | Table | p50 latency | p95 latency | ms/page | $/1k pages | Failed | Dataset |\n");
    out.push_str("|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|\n");
    for (i, (r, m)) in rows.iter().enumerate() {
        let s = &m.summary;
        let opt = |v: Option<f64>| v.map(|x| format!("{x:.3}")).unwrap_or_else(|| "–".into());
        out.push_str(&format!(
            "| {} | `{}` | **{:.2}** | {:.3} | {:.3} | {:.3} | {:.3} | {} | {} | {} ms | {} ms | {:.0} | {} | {}/{} | {} v{} |\n",
            i + 1,
            m.model,
            s.accuracy.overall,
            s.accuracy.char_similarity,
            s.accuracy.cer,
            s.accuracy.wer,
            s.accuracy.word_f1,
            opt(s.accuracy.order_score),
            opt(s.accuracy.table_score),
            s.latency_p50_ms,
            s.latency_p95_ms,
            s.latency_per_page_ms,
            s.cost_per_1k_pages_usd.map(|c| format!("${c:.2}")).unwrap_or_else(|| "–".into()),
            s.accuracy.failed,
            s.accuracy.documents,
            r.dataset.name,
            r.dataset.version,
        ));
    }
    // Per-category breakdown (overall score) if there is more than one category.
    let cats: std::collections::BTreeSet<&String> =
        rows.iter().flat_map(|(_, m)| m.summary.by_category.keys()).collect();
    if cats.len() > 1 {
        out.push_str("\n### Overall score by category\n\n| Model |");
        for c in &cats {
            out.push_str(&format!(" {c} |"));
        }
        out.push_str("\n|---|");
        for _ in &cats {
            out.push_str("---:|");
        }
        out.push('\n');
        for (_, m) in &rows {
            out.push_str(&format!("| `{}` |", m.model));
            for c in &cats {
                match m.summary.by_category.get(*c) {
                    Some(s) => out.push_str(&format!(" {:.1} |", s.overall)),
                    None => out.push_str(" – |"),
                }
            }
            out.push('\n');
        }
    }
    out
}
