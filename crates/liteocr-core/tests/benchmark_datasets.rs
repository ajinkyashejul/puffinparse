//! Self-checks for the adapter-built benchmark datasets under `benchmark/datasets/`.
//!
//! Every rule and every transcript must be satisfiable: a "witness" prediction assembled from a
//! document's own assertions has to pass all of them, and a transcript scored against itself has
//! to score 1.0. A failure here means the adapter emitted a rule the scorer can never pass (a
//! normalisation mismatch, contradictory rules, a malformed table assertion), not a provider
//! problem. The datasets are not shipped with the published crate, so a missing directory skips.

use std::path::{Path, PathBuf};

use liteocr_core::bench::{score, score_rules, NormalizeOptions, Rule, RuleType};

fn datasets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmark/datasets")
}

fn manifest(name: &str) -> Option<(PathBuf, serde_json::Value)> {
    let dir = datasets().join(name);
    let raw = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    Some((dir, serde_json::from_str(&raw).expect("manifest parses")))
}

fn load_rules(path: &Path) -> Vec<Rule> {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// A prediction that satisfies every non-`absent` rule of a document by construction: each
/// `present` text on its own line, each `order` pair in order, each `table_cell` as a one-row
/// markdown table. `absent` texts are left out, so an `absent` failure is a real conflict.
fn witness(rules: &[Rule]) -> String {
    let mut out = Vec::new();
    for r in rules {
        match r.rule_type {
            RuleType::Present => out.push(r.text.clone().unwrap_or_default()),
            RuleType::Order => {
                out.push(r.before.clone().unwrap_or_default());
                out.push(r.after.clone().unwrap_or_default());
            }
            RuleType::BagOfSentences => out.extend(r.sentences.iter().cloned()),
            RuleType::TableCell => {
                let c = r.cell.as_ref().expect("table_cell has a cell");
                let col = c.col_header.as_deref().unwrap_or("col");
                let row = c.row_header.as_deref().unwrap_or("row");
                out.push(format!("| head | {} |\n| --- | --- |\n| {} | {} |", cell(col), cell(row), cell(&c.value)));
            }
            RuleType::Absent => {}
        }
    }
    out.join("\n\n")
}

#[test]
fn olmocr_rules_are_satisfiable() {
    let Some((dir, m)) = manifest("olmocr") else { return };
    let docs = m["documents"].as_array().expect("documents");
    assert!(docs.len() >= 30, "expected the committed olmocr subset, found {}", docs.len());
    let mut total = 0usize;
    let mut problems = Vec::new();
    for d in docs {
        assert_eq!(d["kind"], "rules");
        assert!(dir.join(d["file"].as_str().unwrap()).is_file(), "missing {}", d["file"]);
        let rules = load_rules(&dir.join(d["rules"].as_str().unwrap()));
        assert!(!rules.is_empty());
        total += rules.len();
        let s = score_rules(&witness(&rules), &rules, NormalizeOptions::default());
        if s.passed != s.total {
            problems.push(format!("{}: {:?}", d["id"], s.failures));
        }
        let absent_only = d["tags"].as_array().unwrap().iter().any(|t| t == "absent-only");
        let empty = score_rules("", &rules, NormalizeOptions::default());
        if absent_only {
            assert_eq!(empty.pass_rate, 1.0, "{}: absent-only docs pass for an empty parse", d["id"]);
        } else {
            assert!(empty.pass_rate < 1.0, "{}: an empty parse must not pass", d["id"]);
        }
    }
    assert!(problems.is_empty(), "unsatisfiable rules:\n{}", problems.join("\n"));
    assert!(total >= 150, "expected hundreds of rules, found {total}");
}

#[test]
fn omnidocbench_truth_scores_itself_perfectly() {
    let Some((dir, m)) = manifest("omnidocbench") else { return };
    let docs = m["documents"].as_array().expect("documents");
    assert!(docs.len() >= 30);
    for d in docs {
        assert_eq!(d["kind"], "transcript");
        assert!(d["sha256"].is_string() && d["truth_sha256"].is_string() && d["upstream_path"].is_string());
        // Truth is generated locally by the adapter (research-only data, never committed).
        let Ok(truth) = std::fs::read_to_string(dir.join(d["truth"].as_str().unwrap())) else { continue };
        let s = score(&truth, &truth, NormalizeOptions::default());
        assert_eq!(s.char_similarity, 1.0, "{}", d["id"]);
        let has_table = d["tags"].as_array().unwrap().iter().any(|t| t == "has-table");
        assert_eq!(s.table_score.is_some(), has_table, "{}: table tag disagrees with the truth", d["id"]);
    }
}

#[test]
fn dpbench_truth_scores_itself_perfectly() {
    let Some((dir, m)) = manifest("dpbench") else { return };
    let docs = m["documents"].as_array().expect("documents");
    assert!(docs.len() >= 30, "expected the committed dpbench subset, found {}", docs.len());
    let mut tables = 0;
    for d in docs {
        let id = &d["id"];
        assert_eq!(d["kind"], "transcript", "{id}");
        assert!(d["rules"].is_null(), "{id}: dpbench emits no rules");
        assert!(dir.join(d["file"].as_str().unwrap()).is_file(), "missing {}", d["file"]);
        let truth = std::fs::read_to_string(dir.join(d["truth"].as_str().unwrap())).expect("truth is committed");
        let s = score(&truth, &truth, NormalizeOptions::default());
        assert_eq!(s.char_similarity, 1.0, "{id}");
        let has_table = d["tags"].as_array().unwrap().iter().any(|t| t == "has-table");
        assert_eq!(s.table_score.is_some(), has_table, "{id}: table tag disagrees with the truth");
        if has_table {
            tables += 1;
            assert_eq!(s.table_score, Some(1.0), "{id}");
            assert_eq!(s.teds_grid, Some(1.0), "{id}");
        }
        let empty = score("", &truth, NormalizeOptions::default());
        assert_eq!(empty.char_similarity, 0.0, "{id}: an empty parse must score 0");
    }
    assert!(tables >= 5, "expected table pages in the subset, found {tables}");
}

fn combined_references_resolve(name: &str, n_sources: usize) {
    let Some((dir, m)) = manifest(name) else { return };
    let docs = m["documents"].as_array().expect("documents");
    let sources = m["sources"].as_array().expect("sources");
    assert_eq!(sources.len(), n_sources);
    assert_eq!(docs.len() as u64, sources.iter().map(|s| s["documents"].as_u64().unwrap()).sum::<u64>());
    for d in docs {
        let fetch = d["tags"].as_array().unwrap().iter().any(|t| t == "fetch-required");
        if let Some(rules) = d["rules"].as_str() {
            let rules = load_rules(&dir.join(rules));
            assert!(!rules.is_empty(), "{}", d["id"]);
        }
        if !fetch {
            assert!(dir.join(d["file"].as_str().unwrap()).is_file(), "missing file for {}", d["id"]);
            if let Some(t) = d["truth"].as_str().filter(|t| !t.is_empty()) {
                assert!(dir.join(t).is_file(), "missing truth for {}", d["id"]);
            }
        }
    }
}

#[test]
fn combined_v2_references_resolve() {
    combined_references_resolve("combined-v2", 4);
}

#[test]
fn combined_v3_references_resolve() {
    combined_references_resolve("combined-v3", 5);
}

/// The witness check over an arbitrary directory of rule files, e.g. a full (uncommitted)
/// conversion: `LITEOCR_SELFCHECK_RULES_DIR=<dir> cargo test -p liteocr-core --test
/// benchmark_datasets -- --ignored --nocapture`.
#[test]
#[ignore]
fn rules_dir_is_satisfiable() {
    let Ok(dir) = std::env::var("LITEOCR_SELFCHECK_RULES_DIR") else { return };
    let (mut files, mut rules_n, mut failed) = (0, 0, 0);
    for entry in std::fs::read_dir(&dir).expect("rules dir").flatten() {
        let rules = load_rules(&entry.path());
        let s = score_rules(&witness(&rules), &rules, NormalizeOptions::default());
        files += 1;
        rules_n += rules.len();
        failed += s.failures.len();
        for f in &s.failures {
            println!("{}: {} {}: {}", entry.path().display(), f.rule_type, f.id, f.detail);
        }
    }
    println!("{files} files, {rules_n} rules, {failed} unsatisfiable");
    assert_eq!(failed, 0);
}

/// Score real extractions (e.g. `pdftotext -layout`) of the olmocr PDFs against their rules.
/// Informational: `LITEOCR_SELFCHECK_PRED_DIR=<dir with <id>.txt> cargo test -p liteocr-core
/// --test benchmark_datasets -- --ignored --nocapture`.
#[test]
#[ignore]
fn olmocr_rules_against_extracted_text() {
    let Ok(pred_dir) = std::env::var("LITEOCR_SELFCHECK_PRED_DIR") else { return };
    let Some((dir, m)) = manifest("olmocr") else { return };
    let mut by_type: std::collections::BTreeMap<String, (u32, u32)> = Default::default();
    for d in m["documents"].as_array().unwrap() {
        let id = d["id"].as_str().unwrap();
        let Ok(pred) = std::fs::read_to_string(Path::new(&pred_dir).join(format!("{id}.txt"))) else { continue };
        let rules = load_rules(&dir.join(d["rules"].as_str().unwrap()));
        let s = score_rules(&pred, &rules, NormalizeOptions::default());
        for (t, b) in &s.by_type {
            let e = by_type.entry(t.clone()).or_default();
            e.0 += b.passed;
            e.1 += b.total;
        }
        println!("{id:70} {:.3} ({}/{})", s.pass_rate, s.passed, s.total);
        for f in s.failures.iter().take(3) {
            println!("    {} {}: {}", f.rule_type, f.id, f.detail);
        }
    }
    for (t, (p, n)) in by_type {
        println!("{t:18} {p}/{n}");
    }
}
