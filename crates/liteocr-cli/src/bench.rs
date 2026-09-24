//! `liteocr bench` — run the open benchmark and render reports.

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use futures::stream::{self, StreamExt};
use liteocr_core::bench::{
    metrics_from_rules, score, score_rules, summarize_with, Metrics, NormalizeOptions, Rule, Summary,
};
use liteocr_core::{DocumentRequest, OutputFormat};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod runlog;

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
    /// Continue an interrupted run: skip (model, document) pairs that already have a successful
    /// record in `--out` or its `<out>.partial.jsonl` log, and run only the missing or failed ones.
    /// Use the same `--out` (the default path contains today's date).
    #[arg(long)]
    resume: bool,
    /// Print the plan (documents x models, estimated pages and cost) and exit without any call.
    #[arg(long)]
    dry_run: bool,
    /// Abort before the first call if the estimated cost (manifest pages x list price) exceeds this.
    #[arg(long, value_name = "USD")]
    max_cost: Option<f64>,
    /// Re-issue a document's call this many times after a retryable error (rate limit, 5xx,
    /// timeout, network). Off by default: a retried provider job may be billed twice.
    #[arg(long, default_value_t = 0)]
    retries: u32,
    /// First backoff before a `--retries` re-issue (doubles each time); not a flag.
    #[arg(skip = std::time::Duration::from_secs(2))]
    retry_base: std::time::Duration,
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
    /// Ground-truth markdown, relative to the dataset directory. Empty (and unused) for
    /// `kind: "rules"` documents, so it must tolerate being absent.
    #[serde(default)]
    pub truth: String,
    /// How the document is scored: `"transcript"` (default) or `"rules"`.
    /// See `docs/benchmarks/adapters.md`.
    #[serde(default = "kind_transcript")]
    pub kind: String,
    /// For `kind: "rules"`: the assertion file, relative to the dataset directory.
    #[serde(default)]
    pub rules: Option<String>,
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

fn kind_transcript() -> String {
    KIND_TRANSCRIPT.to_string()
}

const KIND_TRANSCRIPT: &str = "transcript";
const KIND_RULES: &str = "rules";
/// Tag marking a document whose truth is only the page's table, so `table_score` is its
/// primary metric (ParseBench's table split; see `docs/benchmarks/adapters.md`).
const TAG_TABLE_ONLY: &str = "table-only";

impl ManifestDoc {
    fn is_rules(&self) -> bool {
        self.kind == KIND_RULES
    }

    fn table_only(&self) -> bool {
        self.tags.iter().any(|t| t == TAG_TABLE_ONLY)
    }
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DocResult {
    pub id: String,
    pub category: String,
    /// How this document was scored (`"transcript"` or `"rules"`); absent in pre-rules results.
    #[serde(default = "kind_transcript")]
    pub kind: String,
    /// Headlined by `table_score` instead of `char_similarity` (the `table-only` tag).
    #[serde(default)]
    pub table_only: bool,
    pub pages: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `ErrorKind` of a failed document (`"rate_limit"`, `"provider"`, `"timeout"`, …;
    /// `"input"` when the truth or rule file was unreadable). Absent on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    /// The provider's own id for the call (Reducto job id, Extend parse run id, LlamaParse job
    /// id, …), also recorded for failures when the provider had assigned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_job_id: Option<String>,
    /// `true` if the provider reported a result-cache hit, `false` if caches were disabled for
    /// the run, `null` when unknown.
    #[serde(default)]
    pub cache_hit: Option<bool>,
    /// Calls the runner issued for this document (1 unless `--retries` re-issued it). Retries
    /// inside the HTTP client are not counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
    /// RFC 3339 time the first call for this document started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
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
    // Hash manifest + truth + rule files so results are tied to an exact dataset revision.
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(&raw);
    for d in &manifest.documents {
        if let Ok(t) = std::fs::read(dir.join(&d.truth)) {
            h.update(&t);
        }
        if let Some(rules) = &d.rules {
            if let Ok(r) = std::fs::read(dir.join(rules)) {
                h.update(&r);
            }
        }
        if let Ok(f) = std::fs::read(dir.join(&d.file)) {
            h.update(&f);
        }
    }
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok((manifest, hex))
}

/// How the run loop issues one parse call. The CLI uses [`liteocr_core::parse`]; tests inject a
/// fake so the loop, the partial log and resume can be exercised without a network.
pub(crate) type Caller = Arc<
    dyn Fn(DocumentRequest) -> futures::future::BoxFuture<'static, liteocr_core::Result<liteocr_core::ParseResponse>>
        + Send
        + Sync,
>;

async fn run_bench(args: RunArgs) -> Result<()> {
    let caller: Caller = Arc::new(|req| Box::pin(liteocr_core::parse(req)));
    if let Some(result) = run_bench_with(&args, caller).await? {
        println!("{}", render_markdown(&[result]));
    }
    Ok(())
}

/// Settings shared by every call of a run.
#[derive(Clone)]
struct CallCtx {
    dataset: PathBuf,
    timeout: f64,
    norm: NormalizeOptions,
    save: Option<PathBuf>,
    allow_cache: bool,
    retries: u32,
    retry_base: std::time::Duration,
    caller: Caller,
}

/// The whole `bench run`: plan (with `--resume`), budget check, calls with an append-only log,
/// final JSON. Returns `None` for `--dry-run`.
async fn run_bench_with(args: &RunArgs, caller: Caller) -> Result<Option<RunResult>> {
    let wall = std::time::Instant::now();
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
    // The benchmark scores `parse` output, so every model must serve that mode.
    for m in &args.models {
        liteocr_core::ModelRef::parse_for(m, liteocr_core::Mode::Parse)?;
    }
    let norm = NormalizeOptions { case_insensitive: !args.case_sensitive, ..NormalizeOptions::default() };
    eprintln!("dataset {} v{} ({} docs, sha256 {}…)", manifest.name, manifest.version, docs.len(), &sha[..12]);

    let now = chrono::Utc::now();
    let out = args.out.clone().unwrap_or_else(|| {
        PathBuf::from(format!("benchmark/results/{}-{}.json", now.format("%Y-%m-%d"), manifest.name))
    });
    let partial = runlog::partial_path(&out);
    let state = if args.resume {
        let (state, notes) = runlog::load_resume_state(&out, &partial, &sha, &norm)?;
        if notes.is_empty() {
            eprintln!("resume: neither {} nor {} exists; starting a new run", out.display(), partial.display());
        }
        for n in notes {
            eprintln!("resume: {n}");
        }
        state
    } else {
        if partial.exists() && !args.dry_run {
            bail!(
                "{} is left over from an interrupted run; pass --resume to continue it, or delete it to start over",
                partial.display()
            );
        }
        runlog::ResumeState::default()
    };

    let outside =
        state.completed.keys().filter(|(m, id)| !args.models.contains(m) || !docs.iter().any(|d| &d.id == id)).count();
    if outside > 0 {
        eprintln!(
            "resume: warning: {outside} earlier record(s) are outside the current --models/--filter/--limit \
             selection and will not be in the rewritten result"
        );
    }
    let plan = runlog::plan_pairs(&args.models, &docs, &state.completed);
    let estimate = runlog::estimate(&plan, docs.len());
    if args.dry_run {
        println!("{}", estimate.render());
        println!("dry run: no provider was called; results would go to {}", out.display());
        estimate.check_budget(args.max_cost)?;
        return Ok(None);
    }
    estimate.check_budget(args.max_cost)?;
    eprintln!(
        "plan: {} call(s), ~{} page(s), estimated ${:.4} at list price",
        estimate.total_calls(),
        estimate.total_pages(),
        estimate.total_cost_usd()
    );

    let header = runlog::LogHeader {
        run_id: state.run_id.clone().unwrap_or_else(|| uuid_like(&now)),
        created_at: state.created_at.clone().unwrap_or_else(|| now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        dataset_sha256: sha.clone(),
        normalize: norm,
    };
    let mut log = runlog::PartialLog::open(&partial, &header)?;
    let ctx = CallCtx {
        dataset: args.dataset.clone(),
        timeout: args.timeout,
        norm,
        save: args.save_outputs.clone(),
        allow_cache: args.allow_cache,
        retries: args.retries,
        retry_base: args.retry_base,
        caller,
    };
    let resumed: usize = plan.iter().map(|(_, todo)| docs.len() - todo.len()).sum();
    let mut records = state.completed;
    let (mut new_calls, mut new_cost) = (0usize, 0.0f64);

    let mut models = Vec::new();
    for (model, todo) in &plan {
        eprintln!("\n== {model} == ({} to run, {} already done)", todo.len(), docs.len() - todo.len());
        let bar = indicatif::ProgressBar::new(todo.len() as u64);
        bar.set_style(
            indicatif::ProgressStyle::with_template("{msg} [{bar:30}] {pos}/{len} ({elapsed})")
                .unwrap()
                .progress_chars("=> "),
        );
        bar.set_message(model.to_string());
        let mut calls = stream::iter(todo.iter().map(|d| (*d).clone()))
            .map(|doc| {
                let ctx = ctx.clone();
                let model = model.to_string();
                async move { run_doc(&ctx, &doc, &model).await }
            })
            .buffer_unordered(args.concurrency.max(1));
        while let Some(r) = calls.next().await {
            bar.inc(1);
            // Record before anything else so an interrupt after this point loses nothing.
            log.record(model, &r)?;
            new_calls += 1;
            new_cost += r.cost_usd.unwrap_or(0.0);
            records.insert((model.to_string(), r.id.clone()), r);
        }
        bar.finish_and_clear();
        let mut results: Vec<DocResult> =
            docs.iter().filter_map(|d| records.get(&(model.to_string(), d.id.clone())).cloned()).collect();
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
        models.push(ModelResult { model: model.to_string(), docs: results, summary });
    }

    let result = RunResult {
        run_id: header.run_id.clone(),
        created_at: header.created_at.clone(),
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
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, serde_json::to_string_pretty(&result)?)?;
    drop(log);
    std::fs::remove_file(&partial).with_context(|| format!("removing {}", partial.display()))?;
    eprintln!("\nwrote {}", out.display());
    let failed: usize = result.models.iter().map(|m| m.docs.iter().filter(|d| d.error.is_some()).count()).sum();
    let total_cost: f64 = result.models.iter().flat_map(|m| m.docs.iter()).filter_map(|d| d.cost_usd).sum();
    eprintln!("{}", runlog::summary_line(new_calls, resumed, failed, total_cost, new_cost, wall.elapsed()));
    Ok(Some(result))
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

/// What a document is scored against: a reference transcript or a set of assertions.
#[derive(Debug)]
enum Scoring {
    Transcript(String),
    Rules(Vec<Rule>),
}

/// Read a document's ground truth: the truth markdown, or the rule file for `kind: "rules"`.
/// The error string is what lands in [`DocResult::error`], so no provider call is made.
fn load_scoring(dataset: &Path, doc: &ManifestDoc) -> Result<Scoring, String> {
    if doc.is_rules() {
        let rel = doc.rules.as_deref().ok_or_else(|| "rules unreadable: no `rules` path in manifest".to_string())?;
        let raw = std::fs::read_to_string(dataset.join(rel)).map_err(|e| format!("rules unreadable: {e}"))?;
        let rules: Vec<Rule> = serde_json::from_str(&raw).map_err(|e| format!("rules unreadable: {e}"))?;
        Ok(Scoring::Rules(rules))
    } else {
        let truth = std::fs::read_to_string(dataset.join(&doc.truth)).map_err(|e| format!("truth unreadable: {e}"))?;
        Ok(Scoring::Transcript(truth))
    }
}

/// A `DocResult` that recorded a failure (no metrics), preserving the document's identity.
fn failed_doc(doc: &ManifestDoc, kind: &str, error: String) -> DocResult {
    DocResult {
        id: doc.id.clone(),
        category: doc.category.clone(),
        kind: doc.kind.clone(),
        table_only: doc.table_only(),
        pages: doc.pages,
        error: Some(error),
        error_kind: Some(kind.to_string()),
        ..DocResult::default()
    }
}

/// The serde name of an `ErrorKind` (`"rate_limit"`, `"provider"`, …).
fn error_kind_name(kind: liteocr_core::ErrorKind) -> String {
    serde_json::to_value(kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| kind.to_string())
}

/// `true` if the provider reported a cache hit (a `<provider>_cache_hit` metadata key), `false`
/// if caches were disabled for the run, `None` when neither is known.
fn cache_hit(metadata: Option<&BTreeMap<String, serde_json::Value>>, allow_cache: bool) -> Option<bool> {
    let reported =
        metadata.and_then(|m| m.iter().find(|(k, _)| k.ends_with("_cache_hit")).and_then(|(_, v)| v.as_bool()));
    reported.or(if allow_cache { None } else { Some(false) })
}

async fn run_doc(ctx: &CallCtx, doc: &ManifestDoc, model: &str) -> DocResult {
    let started_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let scoring = match load_scoring(&ctx.dataset, doc) {
        Ok(s) => s,
        Err(e) => return DocResult { started_at: Some(started_at), ..failed_doc(doc, "input", e) },
    };
    let mut req = DocumentRequest::from_path(ctx.dataset.join(&doc.file))
        .model(model)
        .timeout_secs(ctx.timeout)
        .output(OutputFormat::Markdown);
    if !ctx.allow_cache {
        if let Some(opts) = cache_busting_options(model) {
            req = req.provider_options(opts);
        }
    }
    let mut attempts = 0u32;
    let outcome = loop {
        attempts += 1;
        match (ctx.caller)(req.clone()).await {
            Err(e) if e.retryable && attempts <= ctx.retries => {
                let delay = ctx.retry_base.saturating_mul(1 << (attempts - 1).min(4));
                tracing::warn!(doc = %doc.id, model, attempts, ?delay, error = %e, "bench: retrying");
                tokio::time::sleep(delay).await;
            }
            other => break other,
        }
    };
    let mut result = match outcome {
        Ok(resp) => {
            if let Some(dir) = &ctx.save {
                // Combined datasets prefix ids with their source (`synthetic/plain_001`), so the
                // output path can have a directory component.
                let path = dir.join(model.replace('/', "_")).join(format!("{}.md", doc.id));
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&path, &resp.markdown) {
                    tracing::warn!(path = %path.display(), error = %e, "bench: could not save output");
                }
                // The unified response next to the markdown gives the viewer blocks and boxes for
                // its layout overlay. Compact JSON: these files are committed with each run.
                match serde_json::to_vec(&resp) {
                    Ok(json) => {
                        let json_path = path.with_extension("json");
                        if let Err(e) = std::fs::write(&json_path, json) {
                            tracing::warn!(path = %json_path.display(), error = %e, "bench: could not save output");
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "bench: could not serialize response"),
                }
            }
            let metrics = match &scoring {
                Scoring::Transcript(truth) => score(&resp.markdown, truth, ctx.norm),
                Scoring::Rules(rules) => metrics_from_rules(&score_rules(&resp.markdown, rules, ctx.norm)),
            };
            DocResult {
                id: doc.id.clone(),
                category: doc.category.clone(),
                kind: doc.kind.clone(),
                table_only: doc.table_only(),
                pages: resp.usage.pages,
                metrics: Some(metrics),
                latency_ms: resp.latency_ms,
                cost_usd: resp.cost_usd,
                provider_job_id: resp.provider_job_id.clone(),
                cache_hit: cache_hit(Some(&resp.metadata), ctx.allow_cache),
                ..DocResult::default()
            }
        }
        Err(e) => DocResult {
            provider_job_id: e.job_id.clone(),
            cache_hit: cache_hit(None, ctx.allow_cache),
            ..failed_doc(doc, &error_kind_name(e.kind), e.to_string())
        },
    };
    result.attempts = Some(attempts);
    result.started_at = Some(started_at);
    result
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Aggregate a set of documents, taking each one's primary metric from its `table_only` flag.
fn summarize_docs<'a>(docs: impl IntoIterator<Item = &'a DocResult>) -> Summary {
    let (metrics, table_only): (Vec<Option<Metrics>>, Vec<bool>) =
        docs.into_iter().map(|d| (d.metrics, d.table_only)).unzip();
    summarize_with(&metrics, &table_only)
}

fn summarize_model(docs: &[DocResult]) -> ModelSummary {
    let accuracy = summarize_docs(docs);
    let mut lat: Vec<u64> = docs.iter().filter(|d| d.error.is_none()).map(|d| d.latency_ms).collect();
    lat.sort_unstable();
    let total_pages: u32 = docs.iter().filter(|d| d.error.is_none()).map(|d| d.pages).sum();
    let total_latency: u64 = lat.iter().sum();
    let total_cost: f64 = docs.iter().filter_map(|d| d.cost_usd).sum();
    let mut by_cat: BTreeMap<String, Vec<&DocResult>> = BTreeMap::new();
    for d in docs {
        by_cat.entry(d.category.clone()).or_default().push(d);
    }
    ModelSummary {
        accuracy,
        latency_p50_ms: percentile(&lat, 0.5),
        latency_p95_ms: percentile(&lat, 0.95),
        latency_per_page_ms: if total_pages == 0 { 0.0 } else { total_latency as f64 / f64::from(total_pages) },
        total_pages,
        total_cost_usd: total_cost,
        cost_per_1k_pages_usd: if total_pages == 0 { None } else { Some(total_cost / f64::from(total_pages) * 1000.0) },
        by_category: by_cat.into_iter().map(|(k, v)| (k, summarize_docs(v))).collect(),
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
    out.push_str("| Rank | Model | Overall | Char sim | CER | WER | Word F1 | Order | Table | Rules | p50 latency | p95 latency | ms/page | $/1k pages | Failed | Dataset |\n");
    out.push_str("|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|\n");
    for (i, (r, m)) in rows.iter().enumerate() {
        let s = &m.summary;
        let opt = |v: Option<f64>| v.map(|x| format!("{x:.3}")).unwrap_or_else(|| "–".into());
        let pct = |v: Option<f64>| v.map(|x| format!("{:.1}%", 100.0 * x)).unwrap_or_else(|| "–".into());
        out.push_str(&format!(
            "| {} | `{}` | **{:.2}** | {:.3} | {:.3} | {:.3} | {:.3} | {} | {} | {} | {} ms | {} ms | {:.0} | {} | {}/{} | {} v{} |\n",
            i + 1,
            m.model,
            s.accuracy.overall,
            s.accuracy.char_similarity,
            s.accuracy.cer,
            s.accuracy.wer,
            s.accuracy.word_f1,
            opt(s.accuracy.order_score),
            opt(s.accuracy.table_score),
            pct(s.accuracy.rule_pass_rate),
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
        // With several datasets in one report the same model appears once per dataset, so the
        // rows need to say which run they came from.
        let datasets: std::collections::BTreeSet<&str> = rows.iter().map(|(r, _)| r.dataset.name.as_str()).collect();
        let multi = datasets.len() > 1;
        out.push_str("\n### Overall score by category\n\n| Model |");
        if multi {
            out.push_str(" Dataset |");
        }
        for c in &cats {
            out.push_str(&format!(" {c} |"));
        }
        out.push_str("\n|---|");
        if multi {
            out.push_str("---|");
        }
        for _ in &cats {
            out.push_str("---:|");
        }
        out.push('\n');
        for (r, m) in &rows {
            out.push_str(&format!("| `{}` |", m.model));
            if multi {
                out.push_str(&format!(" {} |", r.dataset.name));
            }
            for c in &cats {
                match m.summary.by_category.get(*c) {
                    Some(s) => out.push_str(&format!(" {:.1} |", s.overall)),
                    None => out.push_str(" – |"),
                }
            }
            out.push('\n');
        }
    }
    for run in runs {
        out.push_str(&render_by_source(run));
    }
    out
}

/// The source a combined-dataset id came from: everything before the first `/`
/// (`synthetic/plain_001` → `synthetic`). `None` for a plain id.
fn source_of(id: &str) -> Option<&str> {
    id.split_once('/').map(|(source, _)| source)
}

/// Per-source breakdown of one run, for datasets whose ids carry a `source/` prefix
/// (`combined-v1`). Empty for single-source datasets, where it would just restate the main table.
fn render_by_source(run: &RunResult) -> String {
    let sources: std::collections::BTreeSet<&str> =
        run.models.iter().flat_map(|m| m.docs.iter()).filter_map(|d| source_of(&d.id)).collect();
    if sources.len() < 2 {
        return String::new();
    }
    let mut out =
        format!("\n### `{}` by source\n\n| Source | Docs | Model | Overall |\n|---|---:|---|---:|\n", run.dataset.name);
    for source in &sources {
        let mut rows: Vec<(&str, usize, f64)> = run
            .models
            .iter()
            .map(|m| {
                let s = summarize_docs(m.docs.iter().filter(|d| source_of(&d.id) == Some(*source)));
                (m.model.as_str(), s.documents, s.overall)
            })
            .collect();
        rows.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        for (model, documents, overall) in rows {
            out.push_str(&format!("| {source} | {documents} | `{model}` | {overall:.2} |\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_doc_defaults_to_a_transcript() {
        let docs: Vec<ManifestDoc> = serde_json::from_str(
            r#"[{"id": "a", "file": "docs/a.png", "truth": "truth/a.md"},
                {"id": "b", "file": "docs/b.pdf", "truth": "", "kind": "rules",
                 "rules": "rules/b.json", "tags": ["table-only"], "future_key": 1}]"#,
        )
        .expect("manifest documents deserialise");
        assert_eq!(docs[0].kind, "transcript");
        assert!(!docs[0].is_rules() && !docs[0].table_only());
        assert!(docs[0].rules.is_none());
        assert!(docs[1].is_rules() && docs[1].table_only());
        assert_eq!(docs[1].rules.as_deref(), Some("rules/b.json"));
    }

    #[test]
    fn rules_documents_need_no_truth_key() {
        let doc: ManifestDoc =
            serde_json::from_str(r#"{"id": "b", "file": "docs/b.pdf", "kind": "rules", "rules": "r.json"}"#)
                .expect("a missing `truth` is not fatal");
        assert_eq!(doc.truth, "");
        assert_eq!(doc.pages, 1);
    }

    #[test]
    fn missing_rules_file_fails_before_the_provider_call() {
        let doc = ManifestDoc {
            id: "b".into(),
            file: "docs/b.pdf".into(),
            truth: String::new(),
            kind: KIND_RULES.into(),
            rules: Some("does/not/exist.json".into()),
            pages: 1,
            category: "text".into(),
            tags: vec![],
        };
        let err = load_scoring(Path::new("/nonexistent-dataset"), &doc).expect_err("unreadable");
        assert!(err.starts_with("rules unreadable: "), "{err}");
        let no_path = ManifestDoc { rules: None, ..doc };
        assert!(load_scoring(Path::new("/nonexistent-dataset"), &no_path)
            .expect_err("no path")
            .starts_with("rules unreadable: "));
    }

    /// A `table-only` document is headlined by `table_score`, everything else by `char_similarity`.
    #[test]
    fn table_only_documents_are_headlined_by_table_score() {
        let doc = |id: &str, table_only: bool, metrics: Option<Metrics>| DocResult {
            id: id.into(),
            category: "table".into(),
            kind: KIND_TRANSCRIPT.into(),
            table_only,
            pages: 1,
            metrics,
            latency_ms: 1,
            ..DocResult::default()
        };
        let m = Metrics { char_similarity: 0.3, table_score: Some(0.9), ..Metrics::default() };
        let docs = vec![doc("a/x", true, Some(m)), doc("b/y", false, Some(m))];
        let summary = summarize_docs(&docs);
        assert!((summary.overall - 60.0).abs() < 1e-9, "{}", summary.overall);
        assert!((summarize_docs(&docs[..1]).overall - 90.0).abs() < 1e-9);
        assert!((summarize_docs(&docs[1..]).overall - 30.0).abs() < 1e-9);
    }

    #[test]
    fn source_prefix_of_a_combined_id() {
        assert_eq!(source_of("synthetic/plain_001"), Some("synthetic"));
        assert_eq!(source_of("parsebench/text_a/b"), Some("parsebench"));
        assert_eq!(source_of("plain_001"), None);
    }

    /// A result file written before rules existed must still load and render.
    #[test]
    fn pre_rules_result_files_still_load() {
        let raw = r#"{"run_id": "run-1", "created_at": "2026-01-01T00:00:00Z", "liteocr_version": "0.1.0",
            "dataset": {"name": "synthetic-v1", "version": "1.1.0", "documents": 1, "sha256": "ab"},
            "normalize": {"case_insensitive": true, "strip_markdown": true, "strip_punctuation": false},
            "models": [{"model": "reducto/standard",
                "docs": [{"id": "plain_001", "category": "plain", "pages": 1, "latency_ms": 10,
                          "metrics": {"char_similarity": 1.0, "cer": 0.0, "wer": 0.0, "word_recall": 1.0,
                                      "word_precision": 1.0, "word_f1": 1.0, "pred_chars": 3, "truth_chars": 3}}],
                "summary": {"documents": 1, "failed": 0, "char_similarity": 1.0, "cer": 0.0, "wer": 0.0,
                            "word_f1": 1.0, "overall": 100.0, "latency_p50_ms": 10, "latency_p95_ms": 10,
                            "latency_per_page_ms": 10.0, "total_pages": 1, "total_cost_usd": 0.0,
                            "cost_per_1k_pages_usd": 15.0, "by_category": {}}}]}"#;
        let run: RunResult = serde_json::from_str(raw).expect("old result files stay loadable");
        let doc = &run.models[0].docs[0];
        assert_eq!(doc.kind, "transcript");
        assert!(!doc.table_only);
        let md = render_markdown(&[run]);
        assert!(md.contains("| Rules |"), "the report has a Rules column");
        // No rule documents and a single source: `–` in Rules, no per-source table.
        assert!(md.contains(" – |"));
        assert!(!md.contains("by source"));
    }
}
