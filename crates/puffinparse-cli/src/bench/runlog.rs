//! Resume, dry-run and audit plumbing for `puffinparse bench run`.
//!
//! Every finished (model, document) call is appended to `<out>.partial.jsonl` and flushed before
//! the next one is recorded, so a crash or Ctrl-C loses at most the calls that were in flight.
//! `--resume` reads that log (and the final result JSON, if one exists) and plans only the pairs
//! that have no successful record yet. Nothing here scores: metrics come from the run loop and are
//! stored verbatim so a resumed run never re-calls a provider to recover them.

use super::{DocResult, ManifestDoc, RunResult};
use anyhow::{bail, Context, Result};
use puffinparse_core::bench::NormalizeOptions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One line of the partial log. The first line of a log is a `header`; every other line a `doc`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LogLine {
    Header(LogHeader),
    Doc { model: String, doc: Box<DocResult> },
}

/// Identity of the run a partial log belongs to; a resume refuses a log from another dataset
/// revision or normalisation, since its metrics would not be comparable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogHeader {
    pub run_id: String,
    pub created_at: String,
    pub dataset_sha256: String,
    pub normalize: NormalizeOptions,
}

/// `<out>.partial.jsonl` next to the result JSON.
pub fn partial_path(out: &Path) -> PathBuf {
    let mut name = out.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".partial.jsonl");
    out.with_file_name(name)
}

/// Append-only writer for the partial log. Each record is written in one `write_all` and flushed.
pub struct PartialLog {
    file: std::fs::File,
}

impl PartialLog {
    /// Open (creating if needed) the log; writes `header` when the file is new or empty.
    pub fn open(path: &Path, header: &LogHeader) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        let empty = file.metadata().map(|m| m.len() == 0).unwrap_or(true);
        let mut log = Self { file };
        if empty {
            log.append(&LogLine::Header(header.clone()))?;
        }
        Ok(log)
    }

    pub fn append(&mut self, line: &LogLine) -> Result<()> {
        let mut buf = serde_json::to_vec(line)?;
        buf.push(b'\n');
        self.file.write_all(&buf)?;
        self.file.flush()?;
        Ok(())
    }

    pub fn record(&mut self, model: &str, doc: &DocResult) -> Result<()> {
        self.append(&LogLine::Doc { model: model.to_string(), doc: Box::new(doc.clone()) })
    }
}

/// What a partial log holds. A torn last line (the process died mid-write) is skipped and counted.
#[derive(Debug, Default)]
pub struct LogContents {
    pub header: Option<LogHeader>,
    pub records: Vec<(String, DocResult)>,
    pub unreadable_lines: usize,
}

pub fn read_log(path: &Path) -> Result<LogContents> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = LogContents::default();
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<LogLine>(line) {
            Ok(LogLine::Header(h)) => {
                if out.header.is_none() {
                    out.header = Some(h);
                }
            }
            Ok(LogLine::Doc { model, doc }) => out.records.push((model, *doc)),
            Err(_) => out.unreadable_lines += 1,
        }
    }
    Ok(out)
}

/// Successful records by (model, document id). Later records override earlier ones.
pub type Completed = BTreeMap<(String, String), DocResult>;

/// Everything a resume knows: completed pairs plus the run identity to keep.
#[derive(Debug, Default)]
pub struct ResumeState {
    pub completed: Completed,
    pub run_id: Option<String>,
    pub created_at: Option<String>,
}

/// Collect the successful records of a previous (possibly partial) run of the same dataset.
/// `sha`/`norm` are the current run's; a mismatch is an error rather than a silent mix.
pub fn load_resume_state(
    out: &Path,
    partial: &Path,
    sha: &str,
    norm: &NormalizeOptions,
) -> Result<(ResumeState, Vec<String>)> {
    let mut state = ResumeState::default();
    let mut notes = Vec::new();
    let norm_v = serde_json::to_value(norm)?;
    if out.exists() {
        let raw = std::fs::read_to_string(out).with_context(|| format!("reading {}", out.display()))?;
        let run: RunResult = serde_json::from_str(&raw).with_context(|| format!("parsing {}", out.display()))?;
        if run.dataset.sha256 != sha {
            bail!(
                "{} was produced from a different dataset revision (sha256 {}… vs {}…); \
                 rerun without --resume or pick another --out",
                out.display(),
                short(&run.dataset.sha256),
                short(sha)
            );
        }
        if serde_json::to_value(run.normalize)? != norm_v {
            bail!("{} was scored with different normalisation options; rerun without --resume", out.display());
        }
        state.run_id = Some(run.run_id.clone());
        state.created_at = Some(run.created_at.clone());
        let before = state.completed.len();
        absorb(
            &mut state.completed,
            run.models.into_iter().flat_map(|m| m.docs.into_iter().map(move |d| (m.model.clone(), d))),
        );
        notes.push(format!("{}: {} completed pairs", out.display(), state.completed.len() - before));
    }
    if partial.exists() {
        let log = read_log(partial)?;
        if let Some(h) = &log.header {
            if h.dataset_sha256 != sha {
                bail!(
                    "{} belongs to a different dataset revision (sha256 {}… vs {}…); delete it or rerun without --resume",
                    partial.display(),
                    short(&h.dataset_sha256),
                    short(sha)
                );
            }
            if serde_json::to_value(h.normalize)? != norm_v {
                bail!("{} was scored with different normalisation options; delete it to start over", partial.display());
            }
            state.run_id = Some(h.run_id.clone());
            state.created_at = Some(h.created_at.clone());
        }
        let n = log.records.len();
        absorb(&mut state.completed, log.records);
        let mut note = format!("{}: {n} records", partial.display());
        if log.unreadable_lines > 0 {
            note.push_str(&format!(" ({} unreadable line(s) skipped)", log.unreadable_lines));
        }
        notes.push(note);
    }
    Ok((state, notes))
}

/// Merge records into `completed` in order: the latest record for a pair decides, so a success
/// counts as done and a later failure (never produced by a resume, which skips successes) does not.
fn absorb(completed: &mut Completed, records: impl IntoIterator<Item = (String, DocResult)>) {
    for (model, doc) in records {
        let key = (model, doc.id.clone());
        if doc.error.is_none() {
            completed.insert(key, doc);
        } else {
            completed.remove(&key);
        }
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// Pairs still to run for each model, in the order given, skipping completed ones.
pub fn plan_pairs<'a>(
    models: &'a [String],
    docs: &'a [ManifestDoc],
    completed: &Completed,
) -> Vec<(&'a str, Vec<&'a ManifestDoc>)> {
    models
        .iter()
        .map(|m| {
            let todo = docs.iter().filter(|d| !completed.contains_key(&(m.clone(), d.id.clone()))).collect();
            (m.as_str(), todo)
        })
        .collect()
}

/// Estimated spend for one model's planned calls, from the manifest page counts and list prices.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelEstimate {
    pub model: String,
    pub calls: usize,
    pub skipped: usize,
    pub pages: u32,
    pub price_per_page: Option<f64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    pub models: Vec<ModelEstimate>,
}

impl Estimate {
    pub fn total_calls(&self) -> usize {
        self.models.iter().map(|m| m.calls).sum()
    }

    pub fn total_pages(&self) -> u32 {
        self.models.iter().map(|m| m.pages).sum()
    }

    /// Sum of the priced models' estimates.
    pub fn total_cost_usd(&self) -> f64 {
        self.models.iter().filter_map(|m| m.cost_usd).sum()
    }

    /// Models with planned calls but no list price for `parse`.
    pub fn unpriced(&self) -> Vec<&str> {
        self.models.iter().filter(|m| m.calls > 0 && m.cost_usd.is_none()).map(|m| m.model.as_str()).collect()
    }

    /// Enforce `--max-cost` before any call is made.
    pub fn check_budget(&self, max_cost: Option<f64>) -> Result<()> {
        let Some(max) = max_cost else { return Ok(()) };
        let unpriced = self.unpriced();
        if !unpriced.is_empty() {
            bail!(
                "--max-cost {max:.2}: no list price for {} so the estimate is incomplete; \
                 add it to pricing or drop --max-cost",
                unpriced.join(", ")
            );
        }
        let total = self.total_cost_usd();
        if total > max {
            bail!("estimated cost ${total:.4} exceeds --max-cost ${max:.2}; nothing was run");
        }
        Ok(())
    }

    pub fn render(&self) -> String {
        let mut out = String::from("| Model | Calls | Skipped (resumed) | Est. pages | $/page | Est. cost |\n");
        out.push_str("|---|---:|---:|---:|---:|---:|\n");
        let money = |v: Option<f64>, digits: usize| v.map(|x| format!("${x:.digits$}")).unwrap_or_else(|| "–".into());
        for m in &self.models {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} |\n",
                m.model,
                m.calls,
                m.skipped,
                m.pages,
                money(m.price_per_page, 5),
                money(m.cost_usd, 4)
            ));
        }
        out.push_str(&format!(
            "| **total** | {} | {} | {} | | **${:.4}** |\n",
            self.total_calls(),
            self.models.iter().map(|m| m.skipped).sum::<usize>(),
            self.total_pages(),
            self.total_cost_usd()
        ));
        let unpriced = self.unpriced();
        if !unpriced.is_empty() {
            out.push_str(&format!("\nNo list price (not in the total): {}\n", unpriced.join(", ")));
        }
        out
    }
}

/// Estimate the plan's cost. Pages come from the manifest (`pages`, default 1); prices from the
/// same table that fills `cost_usd` on live responses.
pub fn estimate(plan: &[(&str, Vec<&ManifestDoc>)], total_docs: usize) -> Estimate {
    let models = plan
        .iter()
        .map(|(model, todo)| {
            let qualified = puffinparse_core::ModelRef::parse_for(model, puffinparse_core::Mode::Parse)
                .map(|r| r.qualified())
                .unwrap_or_else(|_| (*model).to_string());
            let pages: u32 = todo.iter().map(|d| d.pages).sum();
            let price = puffinparse_core::pricing::price_per_page(&qualified, puffinparse_core::Mode::Parse);
            ModelEstimate {
                model: (*model).to_string(),
                calls: todo.len(),
                skipped: total_docs.saturating_sub(todo.len()),
                pages,
                price_per_page: price,
                cost_usd: price.map(|p| p * f64::from(pages)),
            }
        })
        .collect();
    Estimate { models }
}

/// Final one-line summary of a `bench run` invocation.
pub fn summary_line(
    new_calls: usize,
    resumed: usize,
    failed: usize,
    total_cost: f64,
    new_cost: f64,
    wall: std::time::Duration,
) -> String {
    format!(
        "done: {new_calls} call(s) made, {resumed} resumed, {failed} failed, ${total_cost:.4} total cost \
         (${new_cost:.4} this invocation), wall time {:.1}s",
        wall.as_secs_f64()
    )
}

#[cfg(test)]
mod tests {
    use super::super::{cache_hit, load_manifest, run_bench_with, Caller, DocResult, ManifestDoc, RunArgs, RunResult};
    use super::*;
    use clap::Parser;
    use puffinparse_core::{DocumentRequest, Error, ErrorKind, ParseResponse, Usage};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn doc(id: &str, pages: u32) -> ManifestDoc {
        serde_json::from_value(serde_json::json!({"id": id, "file": format!("docs/{id}.txt"),
            "truth": format!("truth/{id}.md"), "pages": pages, "category": "plain"}))
        .expect("doc")
    }

    fn ok(id: &str) -> DocResult {
        DocResult { id: id.into(), category: "plain".into(), pages: 1, latency_ms: 5, ..DocResult::default() }
    }

    fn failed(id: &str) -> DocResult {
        DocResult { error: Some("provider_error: boom".into()), error_kind: Some("provider".into()), ..ok(id) }
    }

    fn norm() -> NormalizeOptions {
        NormalizeOptions::default()
    }

    /// A unique scratch directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let p = std::env::temp_dir().join(format!("puffinparse-bench-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn partial_path_sits_next_to_the_result() {
        assert_eq!(partial_path(Path::new("a/b/run.json")), PathBuf::from("a/b/run.json.partial.jsonl"));
        assert_eq!(partial_path(Path::new("run.json")), PathBuf::from("run.json.partial.jsonl"));
    }

    #[test]
    fn plan_skips_only_successfully_completed_pairs() {
        let models = vec!["reducto/standard".to_string(), "extend/parse_performance".to_string()];
        let docs = vec![doc("a", 1), doc("b", 2), doc("c", 3)];
        let mut completed = Completed::new();
        absorb(
            &mut completed,
            vec![
                ("reducto/standard".to_string(), ok("a")),
                ("reducto/standard".to_string(), failed("b")),
                ("extend/parse_performance".to_string(), ok("c")),
            ],
        );
        let plan = plan_pairs(&models, &docs, &completed);
        let ids = |i: usize| plan[i].1.iter().map(|d| d.id.as_str()).collect::<Vec<_>>();
        assert_eq!(plan[0].0, "reducto/standard");
        assert_eq!(ids(0), ["b", "c"], "a done; b failed so it is re-run");
        assert_eq!(ids(1), ["a", "b"]);
        // A later success replaces an earlier failure, and a later failure drops a success.
        absorb(&mut completed, vec![("reducto/standard".to_string(), ok("b"))]);
        assert_eq!(plan_pairs(&models, &docs, &completed)[0].1.len(), 1);
        absorb(&mut completed, vec![("reducto/standard".to_string(), failed("a"))]);
        assert_eq!(plan_pairs(&models, &docs, &completed)[0].1.len(), 2);
    }

    #[test]
    fn partial_log_round_trips_and_tolerates_a_torn_line() {
        let dir = Scratch::new("log");
        let path = dir.0.join("sub/run.json.partial.jsonl");
        let sha = "ab".repeat(32);
        let header = LogHeader {
            run_id: "run-1".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            dataset_sha256: sha.clone(),
            normalize: norm(),
        };
        let mut log = PartialLog::open(&path, &header).unwrap();
        let rich = DocResult {
            provider_job_id: Some("job-1".into()),
            cache_hit: Some(false),
            attempts: Some(2),
            started_at: Some("2026-01-01T00:00:01.000Z".into()),
            cost_usd: Some(0.015),
            ..ok("a")
        };
        log.record("reducto/standard", &rich).unwrap();
        log.record("reducto/standard", &failed("b")).unwrap();
        drop(log);
        // Reopening does not write a second header.
        let mut log = PartialLog::open(&path, &header).unwrap();
        log.record("llamaparse/fast", &ok("a")).unwrap();
        drop(log);
        // A crash mid-write leaves a torn last line.
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap() + "{\"type\":\"doc\",\"mod").unwrap();

        let got = read_log(&path).unwrap();
        assert_eq!(got.header.as_ref().unwrap().run_id, "run-1");
        assert_eq!(got.records.len(), 3);
        assert_eq!(got.unreadable_lines, 1);
        let (model, a) = &got.records[0];
        assert_eq!(model, "reducto/standard");
        assert_eq!(a.provider_job_id.as_deref(), Some("job-1"));
        assert_eq!((a.cache_hit, a.attempts, a.cost_usd), (Some(false), Some(2), Some(0.015)));
        assert_eq!(got.records[1].1.error_kind.as_deref(), Some("provider"));
        let headers = std::fs::read_to_string(&path).unwrap().matches("\"type\":\"header\"").count();
        assert_eq!(headers, 1);

        let out = dir.0.join("sub/run.json");
        let (state, notes) = load_resume_state(&out, &path, &sha, &norm()).unwrap();
        assert_eq!(state.run_id.as_deref(), Some("run-1"), "a resume keeps the original run id");
        assert_eq!(state.completed.len(), 2, "a (reducto), a (llamaparse); b failed");
        assert!(notes[0].contains("1 unreadable"), "{notes:?}");
        let err = load_resume_state(&out, &path, "cd", &norm()).unwrap_err();
        assert!(err.to_string().contains("different dataset revision"), "{err}");
        let cs = NormalizeOptions { case_insensitive: !norm().case_insensitive, ..norm() };
        assert!(load_resume_state(&out, &path, &sha, &cs).is_err());
    }

    #[test]
    fn dry_run_estimate_uses_manifest_pages_and_list_prices() {
        let models = vec!["reducto/standard".to_string(), "nope/unpriced".to_string()];
        let docs = vec![doc("a", 1), doc("b", 4)];
        let mut completed = Completed::new();
        completed.insert(("reducto/standard".into(), "a".into()), ok("a"));
        let plan = plan_pairs(&models, &docs, &completed);
        let est = estimate(&plan, docs.len());
        let price =
            puffinparse_core::pricing::price_per_page("reducto/standard", puffinparse_core::Mode::Parse).unwrap();
        let r = &est.models[0];
        assert_eq!((r.calls, r.skipped, r.pages), (1, 1, 4));
        assert!((r.cost_usd.unwrap() - 4.0 * price).abs() < 1e-12);
        assert_eq!(est.models[1].cost_usd, None);
        assert_eq!(est.unpriced(), ["nope/unpriced"]);
        assert_eq!(est.total_pages(), 9);
        assert!((est.total_cost_usd() - 4.0 * price).abs() < 1e-12);
        assert!(est.render().contains("No list price (not in the total): nope/unpriced"));
        assert!(est.check_budget(None).is_ok());
        let err = est.check_budget(Some(1000.0)).unwrap_err().to_string();
        assert!(err.contains("no list price for nope/unpriced"), "{err}");

        let priced = Estimate { models: est.models[..1].to_vec() };
        assert!(priced.check_budget(Some(1.0)).is_ok());
        let err = priced.check_budget(Some(price)).unwrap_err().to_string();
        assert!(err.contains("exceeds --max-cost"), "{err}");
    }

    #[test]
    fn cache_hit_is_reported_disabled_or_unknown() {
        let hit = BTreeMap::from([("llamaparse_cache_hit".to_string(), serde_json::json!(true))]);
        assert_eq!(cache_hit(Some(&hit), true), Some(true));
        assert_eq!(cache_hit(Some(&hit), false), Some(true), "a reported hit wins");
        assert_eq!(cache_hit(Some(&BTreeMap::new()), false), Some(false));
        assert_eq!(cache_hit(Some(&BTreeMap::new()), true), None);
        assert_eq!(cache_hit(None, true), None);
    }

    #[test]
    fn doc_results_without_audit_fields_still_load() {
        let d: DocResult = serde_json::from_str(
            r#"{"id": "a", "category": "plain", "pages": 1, "latency_ms": 3, "error": "timeout_error: slow"}"#,
        )
        .unwrap();
        assert_eq!(
            (d.error_kind, d.provider_job_id, d.cache_hit, d.attempts, d.started_at),
            (None, None, None, None, None)
        );
        // `cache_hit` is always written (null = unknown); the other new fields only when set.
        let v = serde_json::to_value(ok("a")).unwrap();
        assert!(v.get("cache_hit").unwrap().is_null());
        assert!(v.get("provider_job_id").is_none() && v.get("attempts").is_none());
    }

    // ---- the run loop, end to end, with a fake provider -----------------------------------------

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        run: RunArgs,
    }

    fn args(dir: &Path, extra: &[&str]) -> RunArgs {
        let mut argv = vec![
            "t".to_string(),
            "--dataset".into(),
            dir.join("ds").display().to_string(),
            "--models".into(),
            "reducto/standard".into(),
            "--out".into(),
            dir.join("out/run.json").display().to_string(),
        ];
        argv.extend(extra.iter().map(|s| s.to_string()));
        let mut a = Cli::try_parse_from(argv).expect("args").run;
        a.retry_base = std::time::Duration::from_millis(1);
        a
    }

    fn write_dataset(dir: &Path) {
        let ds = dir.join("ds");
        std::fs::create_dir_all(ds.join("docs")).unwrap();
        std::fs::create_dir_all(ds.join("truth")).unwrap();
        let mut documents = Vec::new();
        for id in ["a", "b", "c"] {
            std::fs::write(ds.join(format!("docs/{id}.txt")), id).unwrap();
            std::fs::write(ds.join(format!("truth/{id}.md")), format!("hello {id}")).unwrap();
            documents.push(serde_json::json!({"id": id, "file": format!("docs/{id}.txt"),
                "truth": format!("truth/{id}.md"), "pages": 2, "category": "plain"}));
        }
        let manifest = serde_json::json!({"name": "tiny", "version": "1", "documents": documents});
        std::fs::write(ds.join("manifest.json"), manifest.to_string()).unwrap();
    }

    /// Decrements `n` if it is positive; true when a unit was taken. A compare-and-swap loop rather
    /// than `fetch_update`, which newer toolchains deprecate and whose replacement postdates the MSRV.
    fn take_one(n: &AtomicUsize) -> bool {
        let mut current = n.load(Ordering::SeqCst);
        while current > 0 {
            match n.compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
        false
    }

    /// Fake provider: `a` succeeds, `b` fails `b_failures` times with a retryable error then
    /// succeeds, `c` fails with a non-retryable error carrying a job id. Records calls by doc id.
    fn fake(calls: Arc<Mutex<Vec<String>>>, b_failures: usize) -> Caller {
        let b_failures = Arc::new(AtomicUsize::new(b_failures));
        Arc::new(move |req: DocumentRequest| {
            let calls = calls.clone();
            let b_failures = b_failures.clone();
            Box::pin(async move {
                let path = req.input.describe();
                let id = Path::new(&path).file_stem().unwrap().to_string_lossy().to_string();
                calls.lock().unwrap().push(id.clone());
                if id == "b" && take_one(&b_failures) {
                    return Err(Error::new(ErrorKind::RateLimit, "slow down")
                        .with_provider("reducto")
                        .with_status(429));
                }
                if id == "c" {
                    return Err(Error::new(ErrorKind::BadRequest, "bad pdf")
                        .with_provider("reducto")
                        .with_job_id("job-c"));
                }
                let usage = Usage { pages: 2, ..Usage::default() };
                let mut resp = ParseResponse::from_pages("reducto", "standard", vec![], usage);
                resp.markdown = format!("hello {id}");
                resp.provider_job_id = Some(format!("job-{id}"));
                resp.cost_usd = Some(0.03);
                resp.latency_ms = 7;
                Ok(resp)
            })
        })
    }

    fn load(path: &Path) -> RunResult {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn called(calls: &Mutex<Vec<String>>) -> Vec<String> {
        let mut v = calls.lock().unwrap().clone();
        v.sort();
        v
    }

    #[tokio::test]
    async fn save_outputs_writes_markdown_and_unified_json() {
        let dir = Scratch::new("save");
        write_dataset(&dir.0);
        let saved = dir.0.join("outputs");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let extra = ["--save-outputs", saved.to_str().unwrap(), "--filter", "a"];
        run_bench_with(&args(&dir.0, &extra), fake(calls, 0)).await.unwrap().expect("ran");
        let base = saved.join("reducto_standard");
        assert_eq!(std::fs::read_to_string(base.join("a.md")).unwrap(), "hello a");
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(base.join("a.json")).unwrap()).unwrap();
        assert_eq!(json["markdown"], "hello a");
        assert_eq!(json["provider_job_id"], "job-a");
    }

    #[tokio::test]
    async fn run_records_audit_fields_and_resume_reruns_only_missing_pairs() {
        let dir = Scratch::new("run");
        write_dataset(&dir.0);
        let out = dir.0.join("out/run.json");
        let partial = partial_path(&out);
        let calls = Arc::new(Mutex::new(Vec::new()));

        // 1. Full run: b needs one retry, c fails for good.
        let result =
            run_bench_with(&args(&dir.0, &["--retries", "1"]), fake(calls.clone(), 1)).await.unwrap().expect("ran");
        assert!(!partial.exists(), "the partial log is removed once the result JSON is written");
        let docs = &load(&out).models[0].docs;
        assert_eq!(docs.len(), 3);
        let (a, b, c) = (&docs[0], &docs[1], &docs[2]);
        assert_eq!(a.provider_job_id.as_deref(), Some("job-a"));
        assert_eq!((a.attempts, a.cache_hit, a.error.as_ref()), (Some(1), Some(false), None));
        assert!(a.started_at.is_some() && a.metrics.unwrap().char_similarity > 0.99);
        assert_eq!(b.attempts, Some(2), "one retry after the 429");
        assert!(b.error.is_none());
        assert_eq!(c.error_kind.as_deref(), Some("bad_request"));
        assert!(c.error.as_deref().unwrap().contains("bad pdf"), "the provider message is kept");
        assert_eq!(c.provider_job_id.as_deref(), Some("job-c"));
        assert_eq!(c.attempts, Some(1), "non-retryable errors are not re-issued");

        // 2. Resume from the final JSON: only the failed pair is called again.
        calls.lock().unwrap().clear();
        let resumed = run_bench_with(&args(&dir.0, &["--resume"]), fake(calls.clone(), 0)).await.unwrap().unwrap();
        assert_eq!(called(&calls), ["c"]);
        assert_eq!(resumed.run_id, result.run_id);
        assert_eq!(resumed.models[0].docs.len(), 3);
        assert_eq!(resumed.models[0].docs[0].provider_job_id.as_deref(), Some("job-a"), "kept from run 1");

        // 3. Simulated crash: a partial log with only `a` done, and no result JSON.
        std::fs::remove_file(&out).unwrap();
        let header = LogHeader {
            run_id: "run-crashed".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            dataset_sha256: load_manifest(&dir.0.join("ds")).unwrap().1,
            normalize: resumed.normalize,
        };
        let mut log = PartialLog::open(&partial, &header).unwrap();
        log.record("reducto/standard", &DocResult { provider_job_id: Some("job-a-old".into()), ..ok("a") }).unwrap();
        drop(log);

        // Without --resume the leftover log is refused rather than overwritten.
        let err = run_bench_with(&args(&dir.0, &[]), fake(calls.clone(), 0)).await.unwrap_err();
        assert!(err.to_string().contains("pass --resume"), "{err}");

        // A dry run of the resume makes no call; --max-cost below the estimate aborts first.
        calls.lock().unwrap().clear();
        let dry = run_bench_with(&args(&dir.0, &["--resume", "--dry-run"]), fake(calls.clone(), 0)).await.unwrap();
        assert!(dry.is_none());
        let err = run_bench_with(&args(&dir.0, &["--resume", "--max-cost", "0.0001"]), fake(calls.clone(), 0))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exceeds --max-cost"), "{err}");
        assert!(calls.lock().unwrap().is_empty());
        assert!(partial.exists() && !out.exists());

        let resumed = run_bench_with(&args(&dir.0, &["--resume"]), fake(calls.clone(), 0)).await.unwrap().unwrap();
        assert_eq!(called(&calls), ["b", "c"]);
        assert_eq!(resumed.run_id, "run-crashed");
        assert_eq!(resumed.models[0].docs[0].provider_job_id.as_deref(), Some("job-a-old"));
        assert!(!partial.exists());
    }
}
