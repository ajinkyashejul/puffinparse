//! Benchmark metrics: text normalisation and accuracy scores between a prediction and ground truth.
//!
//! All metrics are deterministic and need no model or network access.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

pub mod tables;

pub use tables::{extract_tables, teds_grid, Grid};

/// Version of the scoring pipeline (normalisation, metrics, rule semantics).
///
/// Recorded in every result JSON as `scorer_version` and bumped whenever a change would move a
/// committed score, so `liteocr bench rescore` can bring old runs up to date offline. Result files
/// written before the field existed are version `1`.
///
/// * `1` — initial scorer (pipe tables only, exact rule matching, headline in `char_similarity`).
/// * `2` — HTML tables, `teds_grid`, punctuation-spacing-insensitive rule matching, fuzzy
///   `bag_of_sentences`, entity decoding, explicit `Summary::score`.
pub const SCORER_VERSION: u32 = 2;

/// A `bag_of_sentences` sentence counts as present when some window of the prediction is within
/// this normalised character similarity of it (`1 - edit_distance / |sentence|`).
///
/// ParseBench's sentences are cut from its own reference extraction, so they carry that
/// extraction's small artifacts (a hyphenation, a stray footnote marker, a dropped space). 0.8
/// tolerates about one edit in five characters: enough for those artifacts, not enough for a
/// different sentence to pass (two unrelated English sentences of equal length sit well below 0.5).
pub const BAG_SENTENCE_MIN_SIMILARITY: f64 = 0.8;

/// Fraction of a bag's sentences that must be present when the rule sets no `threshold`. The
/// ParseBench adapter writes the same value explicitly.
///
/// A bag is every sentence of the page as cut by the publisher's reference extraction, which fuses
/// neighbouring lines into one "sentence" now and then (`"j i 25 k"`). At 1.0 a single such
/// artifact fails the rule for every model, which made it dead signal; at 0.8 it still fails output
/// that lost a column, a script or most of the page.
pub const BAG_DEFAULT_THRESHOLD: f64 = 0.8;

/// Options for [`normalize`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NormalizeOptions {
    /// Lowercase everything.
    pub case_insensitive: bool,
    /// Strip markdown syntax (headings, emphasis, table pipes, list bullets).
    pub strip_markdown: bool,
    /// Drop all punctuation.
    pub strip_punctuation: bool,
}

impl Default for NormalizeOptions {
    fn default() -> Self {
        Self { case_insensitive: true, strip_markdown: true, strip_punctuation: false }
    }
}

/// Normalise text so that formatting differences don't dominate the score:
/// NFKC, optional markdown stripping, whitespace collapsing, optional lowercasing.
pub fn normalize(s: &str, opts: NormalizeOptions) -> String {
    let s: String = s.nfkc().collect();
    // Tags are stripped before entities are decoded, so `&lt;b&gt;` survives as text.
    let s = if opts.strip_markdown { tables::decode_entities(&crate::types::markdown_to_text(&s)) } else { s };
    let s = s
        .replace(['\u{2018}', '\u{2019}'], "'")
        .replace(['\u{201C}', '\u{201D}'], "\"")
        .replace(['\u{2013}', '\u{2014}'], "-");
    let s = if opts.case_insensitive { s.to_lowercase() } else { s };
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else if opts.strip_punctuation && c.is_ascii_punctuation() {
            continue;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out.trim().to_string()
}

/// Scores for one (prediction, truth) pair. All in `0..=1` unless noted.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Metrics {
    /// `1 - levenshtein / max(len)`. Primary score.
    pub char_similarity: f64,
    /// Character error rate: `levenshtein / |truth|` (can exceed 1).
    pub cer: f64,
    /// Word error rate: `word_levenshtein / |truth_words|` (can exceed 1).
    pub wer: f64,
    /// Fraction of truth word tokens (multiset) found in the prediction.
    pub word_recall: f64,
    /// Fraction of prediction word tokens (multiset) found in the truth.
    pub word_precision: f64,
    /// Harmonic mean of word recall and precision.
    pub word_f1: f64,
    /// Reading-order agreement of shared lines (1 = same order). `None` if < 2 shared lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_score: Option<f64>,
    /// Similarity restricted to table rows (markdown pipe tables and HTML `<table>`s, both reduced
    /// to rows of cells). `None` if the truth has no tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_score: Option<f64>,
    /// TEDS over the row/cell grid of the tables ([`tables::teds_grid`]): table *structure* plus
    /// cell content. `None` if the truth has no tables (and in result files older than scorer v2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teds_grid: Option<f64>,
    pub pred_chars: usize,
    pub truth_chars: usize,
    /// `passed / total` of a rule-scored document ([`score_rules`]). `None` for transcript documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_pass_rate: Option<f64>,
    /// Rules that passed, for a rule-scored document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_passed: Option<u32>,
    /// Rules checked, for a rule-scored document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_total: Option<u32>,
}

/// Compute all metrics for a prediction vs truth (both raw; normalisation applied internally).
pub fn score(pred: &str, truth: &str, opts: NormalizeOptions) -> Metrics {
    let p = normalize(pred, opts);
    let t = normalize(truth, opts);
    let p_chars: Vec<char> = p.chars().collect();
    let t_chars: Vec<char> = t.chars().collect();
    let dist = levenshtein(&p_chars, &t_chars);
    let max_len = p_chars.len().max(t_chars.len());
    let char_similarity = if max_len == 0 { 1.0 } else { 1.0 - dist as f64 / max_len as f64 };
    let cer = if t_chars.is_empty() {
        if p_chars.is_empty() {
            0.0
        } else {
            1.0
        }
    } else {
        dist as f64 / t_chars.len() as f64
    };

    let pw: Vec<&str> = p.split(' ').filter(|w| !w.is_empty()).collect();
    let tw: Vec<&str> = t.split(' ').filter(|w| !w.is_empty()).collect();
    let wdist = levenshtein(&pw, &tw);
    let wer = if tw.is_empty() {
        if pw.is_empty() {
            0.0
        } else {
            1.0
        }
    } else {
        wdist as f64 / tw.len() as f64
    };
    let (word_recall, word_precision) = bag_overlap(&pw, &tw);
    let word_f1 = if word_recall + word_precision == 0.0 {
        0.0
    } else {
        2.0 * word_recall * word_precision / (word_recall + word_precision)
    };

    let order_score = order_agreement(pred, truth, opts);
    let (table_score, teds_grid) = table_scores(pred, truth, opts);

    Metrics {
        char_similarity,
        cer,
        wer,
        word_recall,
        word_precision,
        word_f1,
        order_score,
        table_score,
        teds_grid,
        pred_chars: p_chars.len(),
        truth_chars: t_chars.len(),
        rule_pass_rate: None,
        rules_passed: None,
        rules_total: None,
    }
}

/// Generic Levenshtein distance over any equatable sequence (O(n·m) time, O(min) memory).
pub fn levenshtein<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let (a, b) = if a.len() < b.len() { (b, a) } else { (a, b) };
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Multiset overlap → (recall, precision).
fn bag_overlap(pred: &[&str], truth: &[&str]) -> (f64, f64) {
    use std::collections::HashMap;
    if truth.is_empty() && pred.is_empty() {
        return (1.0, 1.0);
    }
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for w in truth {
        *counts.entry(w).or_default() += 1;
    }
    let mut hits = 0usize;
    for w in pred {
        if let Some(c) = counts.get_mut(w) {
            if *c > 0 {
                *c -= 1;
                hits += 1;
            }
        }
    }
    let recall = if truth.is_empty() { 0.0 } else { hits as f64 / truth.len() as f64 };
    let precision = if pred.is_empty() { 0.0 } else { hits as f64 / pred.len() as f64 };
    (recall, precision)
}

/// Kendall-τ-style order agreement between lines that appear in both texts.
fn order_agreement(pred: &str, truth: &str, opts: NormalizeOptions) -> Option<f64> {
    let norm_lines =
        |s: &str| -> Vec<String> { s.lines().map(|l| normalize(l, opts)).filter(|l| l.chars().count() >= 4).collect() };
    let t_lines = norm_lines(truth);
    let p_lines = norm_lines(pred);
    // position of first occurrence of each truth line in pred
    let mut positions: Vec<usize> = Vec::new();
    let mut used = vec![false; p_lines.len()];
    for tl in &t_lines {
        if let Some(idx) = p_lines.iter().enumerate().position(|(i, pl)| !used[i] && pl == tl) {
            used[idx] = true;
            positions.push(idx);
        }
    }
    if positions.len() < 2 {
        return None;
    }
    let mut concordant = 0usize;
    let mut total = 0usize;
    for i in 0..positions.len() {
        for j in i + 1..positions.len() {
            total += 1;
            if positions[i] < positions[j] {
                concordant += 1;
            }
        }
    }
    Some(concordant as f64 / total as f64)
}

/// Every table of a document (markdown and HTML) with each cell normalised.
fn normalized_tables(s: &str, opts: NormalizeOptions) -> Vec<Grid> {
    extract_tables(s)
        .into_iter()
        .map(|t| t.into_iter().map(|row| row.iter().map(|c| normalize(c, opts)).collect()).collect())
        .collect()
}

/// `(table_score, teds_grid)`, both `None` if the truth has no table.
///
/// `table_score` is the character similarity of the table rows only — each row's cells joined by a
/// space, rows by a newline — so text around the tables does not count. `teds_grid` compares the
/// same tables as trees ([`tables::teds_grid`]).
fn table_scores(pred: &str, truth: &str, opts: NormalizeOptions) -> (Option<f64>, Option<f64>) {
    let t_tables = normalized_tables(truth, opts);
    if t_tables.is_empty() {
        return (None, None);
    }
    let p_tables = normalized_tables(pred, opts);
    let rows = |tables: &[Grid]| -> Vec<char> {
        tables
            .iter()
            .flatten()
            .map(|row| row.iter().flat_map(|c| c.split_whitespace()).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .collect()
    };
    let (tj, pj) = (rows(&t_tables), rows(&p_tables));
    let max_len = tj.len().max(pj.len());
    let table_score = if max_len == 0 { 1.0 } else { 1.0 - levenshtein(&pj, &tj) as f64 / max_len as f64 };
    (Some(table_score), teds_grid(&p_tables, &t_tables))
}

/// Aggregate of per-document metrics.
///
/// `headline` is the number leaderboards rank on: the mean over documents of each document's primary metric
/// ([`headline`]: `table_score` for `table-only` documents, the pass rate for rule documents,
/// `char_similarity` otherwise), failures counting 0; `overall = 100 × headline`. `char_similarity` is
/// the plain mean of the documents' `char_similarity` (for a rule document that field carries the
/// pass rate, see [`metrics_from_rules`]).
///
/// Deserialising a result file written before `headline` existed (scorer v1) fills it from
/// `overall / 100`; in those files `char_similarity` held the headline too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(from = "SummaryWire")]
pub struct Summary {
    pub documents: usize,
    pub failed: usize,
    /// Headline accuracy in `0..=1` (see the type docs). Leaderboards rank on this.
    pub headline: f64,
    /// Mean of per-document `char_similarity`; failures count as 0.
    pub char_similarity: f64,
    pub cer: f64,
    pub wer: f64,
    pub word_f1: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_score: Option<f64>,
    /// Mean `teds_grid` over the documents whose truth has a table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teds_grid: Option<f64>,
    /// Mean rule pass rate over the documents that were rule-scored. `None` if there were none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_pass_rate: Option<f64>,
    /// `100 * headline`; failures count as 0.
    pub overall: f64,
}

/// The wire shape of [`Summary`], tolerant of files that predate `score`.
#[derive(Deserialize)]
struct SummaryWire {
    documents: usize,
    failed: usize,
    #[serde(default)]
    headline: Option<f64>,
    char_similarity: f64,
    cer: f64,
    wer: f64,
    word_f1: f64,
    #[serde(default)]
    order_score: Option<f64>,
    #[serde(default)]
    table_score: Option<f64>,
    #[serde(default)]
    teds_grid: Option<f64>,
    #[serde(default)]
    rule_pass_rate: Option<f64>,
    overall: f64,
}

impl From<SummaryWire> for Summary {
    fn from(w: SummaryWire) -> Self {
        Self {
            documents: w.documents,
            failed: w.failed,
            headline: w.headline.unwrap_or(w.overall / 100.0),
            char_similarity: w.char_similarity,
            cer: w.cer,
            wer: w.wer,
            word_f1: w.word_f1,
            order_score: w.order_score,
            table_score: w.table_score,
            teds_grid: w.teds_grid,
            rule_pass_rate: w.rule_pass_rate,
            overall: w.overall,
        }
    }
}

/// Mean over documents. `failed` documents (no metrics) count as zero accuracy.
pub fn summarize(metrics: &[Option<Metrics>]) -> Summary {
    summarize_with(metrics, &[])
}

/// The primary metric of one document.
///
/// `table-only` documents (ParseBench's table split: the truth is the page's *table*, the
/// prediction is the whole page) must be scored on `table_score`; everything else on
/// `char_similarity` (which [`metrics_from_rules`] sets to the pass rate for rule documents). See
/// `docs/benchmarks/adapters.md`.
pub fn headline(metrics: &Metrics, table_only: bool) -> f64 {
    if table_only {
        metrics.table_score.unwrap_or(metrics.char_similarity)
    } else {
        metrics.char_similarity
    }
}

/// [`summarize`], but taking each document's primary metric from [`headline`].
///
/// `table_only[i]` flags document `i`; a shorter slice (including an empty one, which is what
/// [`summarize`] passes) means `false`, so the two agree whenever no document is table-only.
pub fn summarize_with(metrics: &[Option<Metrics>], table_only: &[bool]) -> Summary {
    let n = metrics.len();
    let headlines: Vec<f64> = metrics
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.as_ref().map(|m| headline(m, table_only.get(i).copied().unwrap_or(false))))
        .collect();
    let ok: Vec<&Metrics> = metrics.iter().flatten().collect();
    let failed = n - ok.len();
    let mean = |f: &dyn Fn(&Metrics) -> f64| -> f64 {
        if n == 0 {
            0.0
        } else {
            ok.iter().map(|m| f(m)).sum::<f64>() / n as f64
        }
    };
    let mean_opt = |f: &dyn Fn(&Metrics) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = ok.iter().filter_map(|m| f(m)).collect();
        if vals.is_empty() {
            None
        } else {
            Some(vals.iter().sum::<f64>() / vals.len() as f64)
        }
    };
    let headline = if n == 0 { 0.0 } else { headlines.iter().sum::<f64>() / n as f64 };
    Summary {
        documents: n,
        failed,
        headline,
        char_similarity: mean(&|m| m.char_similarity),
        // error rates: failures count as 1.0
        cer: if n == 0 { 0.0 } else { (ok.iter().map(|m| m.cer).sum::<f64>() + failed as f64) / n as f64 },
        wer: if n == 0 { 0.0 } else { (ok.iter().map(|m| m.wer).sum::<f64>() + failed as f64) / n as f64 },
        word_f1: mean(&|m| m.word_f1),
        order_score: mean_opt(&|m| m.order_score),
        table_score: mean_opt(&|m| m.table_score),
        teds_grid: mean_opt(&|m| m.teds_grid),
        rule_pass_rate: mean_opt(&|m| m.rule_pass_rate),
        overall: 100.0 * headline,
    }
}

// ---- rule-based scoring -------------------------------------------------------------------------
//
// Some benchmarks (ParseBench today, olmOCR-bench next) ship machine-checkable assertions instead
// of a reference transcript: `kind: "rules"` documents in `docs/benchmarks/adapters.md`. A document
// scores `passed / total`, which has the same shape as an accuracy in `0..=1` and therefore slots
// into [`Metrics`] via [`metrics_from_rules`].

/// What a [`Rule`] asserts about the parsed markdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleType {
    /// The prediction contains `text`.
    Present,
    /// The prediction does *not* contain `text`.
    Absent,
    /// The prediction contains `before` and then `after`, in that order.
    Order,
    /// The prediction has a table (markdown or HTML) whose matching cell equals `cell.value`.
    TableCell,
    /// The prediction contains at least `threshold` of `sentences`, each matched fuzzily
    /// ([`BAG_SENTENCE_MIN_SIMILARITY`]).
    BagOfSentences,
}

impl RuleType {
    /// The wire name, identical to the serde representation (`"table_cell"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            RuleType::Present => "present",
            RuleType::Absent => "absent",
            RuleType::Order => "order",
            RuleType::TableCell => "table_cell",
            RuleType::BagOfSentences => "bag_of_sentences",
        }
    }
}

impl std::fmt::Display for RuleType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The cell assertion of a [`RuleType::TableCell`] rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CellRule {
    /// Row label to look for; `None` matches any row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_header: Option<String>,
    /// Column label to look for in the table header row; `None` matches any cell of the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col_header: Option<String>,
    /// The value the cell must hold.
    pub value: String,
}

/// One machine-checkable assertion about a parsed document.
///
/// Mirrors the rule schema in `docs/benchmarks/adapters.md`. Unknown fields are ignored so that a
/// newer adapter can add keys without breaking an older binary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Stable id of the rule inside its document.
    #[serde(default)]
    pub id: String,
    /// Which assertion this is (`type` on the wire).
    #[serde(rename = "type")]
    pub rule_type: RuleType,
    /// `present` / `absent`: the text that must (not) appear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `order`: the text that must come first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// `order`: the text that must come second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// `table_cell`: the cell assertion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<CellRule>,
    /// `bag_of_sentences`: the sentences to look for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sentences: Vec<String>,
    /// `bag_of_sentences`: required pass fraction, default [`BAG_DEFAULT_THRESHOLD`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Compare case-sensitively; overrides [`NormalizeOptions::case_insensitive`] for this rule.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Levenshtein edits tolerated when matching `text` / `before` / `after` / table headers and
    /// values (olmOCR-bench's `max_diffs`, which upstream applies with `fuzzysearch`). `None` or `0`
    /// means exact matching after normalisation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_diffs: Option<usize>,
    /// The upstream rule id, so a score can be pushed back to the publisher's own harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Passed / total for one [`RuleType`] bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuleTypeScore {
    pub passed: u32,
    pub total: u32,
}

impl RuleTypeScore {
    /// `passed / total`, or `None` when the bucket is empty.
    pub fn rate(&self) -> Option<f64> {
        (self.total > 0).then(|| f64::from(self.passed) / f64::from(self.total))
    }
}

/// A rule that did not pass, with a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleFailure {
    pub id: String,
    pub rule_type: String,
    pub detail: String,
}

/// The result of [`score_rules`]: `passed / total` overall plus per-type buckets and the failures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RuleScore {
    pub passed: u32,
    pub total: u32,
    /// `passed / total`; `1.0` for an empty rule set (vacuously true).
    pub pass_rate: f64,
    /// Keyed by the wire name of [`RuleType`] (`"present"`, `"table_cell"`, …).
    pub by_type: BTreeMap<String, RuleTypeScore>,
    /// Every failing rule, in input order. Callers that serialise this may want to truncate it:
    /// a document can carry thousands of rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<RuleFailure>,
}

/// One table of the prediction (markdown or HTML), with every cell already rule-normalised.
#[derive(Debug, Clone, Default)]
struct Table {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// Is `c` punctuation for [`squeeze_punct_spaces`]? ASCII punctuation plus the Unicode quote,
/// dash, bracket and CJK punctuation ranges (NFKC has already folded full-width forms to ASCII).
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(c,
            '\u{a1}' | '\u{a7}' | '\u{ab}' | '\u{b6}' | '\u{b7}' | '\u{bb}' | '\u{bf}'
            | '\u{2010}'..='\u{2027}'
            | '\u{2030}'..='\u{205e}'
            | '\u{3001}'..='\u{3003}'
            | '\u{3008}'..='\u{3011}'
            | '\u{3014}'..='\u{301f}'
            | '\u{0964}' | '\u{0965}' // Devanagari/Bengali danda
        )
}

/// Drop every space that touches punctuation: `(this " agreement ")` → `(this"agreement")`.
///
/// ParseBench's rule text comes from a tokenised reference extraction, so quotes, brackets and
/// commas are often spaced as separate tokens while the page (and a correct parse) has them tight
/// — or the other way round. Applied to both the prediction and the rule text, this makes spaces
/// adjacent to punctuation insignificant for rule matching, and nothing else. Input must already
/// be whitespace-collapsed ([`normalize`]).
fn squeeze_punct_spaces(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' {
            let prev = i.checked_sub(1).map(|j| chars[j]);
            let next = chars.get(i + 1).copied();
            if prev.is_some_and(is_punct) || next.is_some_and(is_punct) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// [`normalize`] followed by [`squeeze_punct_spaces`]: the normalisation every rule comparison uses.
fn rule_normalize(s: &str, opts: NormalizeOptions) -> String {
    squeeze_punct_spaces(&normalize(s, opts))
}

/// The prediction, normalised at most once per case-sensitivity variant.
///
/// Rules carry their own `case_sensitive` flag, so a document needs at most two normalised copies
/// (and at most two parsed table sets) no matter how many thousands of rules it has.
#[derive(Debug)]
struct PredictionCache<'a> {
    raw: &'a str,
    opts: NormalizeOptions,
    text: [std::cell::OnceCell<String>; 2],
    chars: [std::cell::OnceCell<Vec<char>>; 2],
    tables: [std::cell::OnceCell<Vec<Table>>; 2],
}

impl<'a> PredictionCache<'a> {
    fn new(raw: &'a str, opts: NormalizeOptions) -> Self {
        Self { raw, opts, text: Default::default(), chars: Default::default(), tables: Default::default() }
    }

    /// Normalisation for a rule: the run's options with `case_insensitive` taken from the rule.
    fn opts_for(&self, case_sensitive: bool) -> NormalizeOptions {
        NormalizeOptions { case_insensitive: !case_sensitive, ..self.opts }
    }

    fn text(&self, case_sensitive: bool) -> &str {
        self.text[usize::from(case_sensitive)].get_or_init(|| rule_normalize(self.raw, self.opts_for(case_sensitive)))
    }

    /// [`Self::text`] as chars, for fuzzy matching.
    fn chars(&self, case_sensitive: bool) -> &[char] {
        self.chars[usize::from(case_sensitive)].get_or_init(|| self.text(case_sensitive).chars().collect())
    }

    fn tables(&self, case_sensitive: bool) -> &[Table] {
        self.tables[usize::from(case_sensitive)].get_or_init(|| {
            let opts = self.opts_for(case_sensitive);
            extract_tables(self.raw)
                .into_iter()
                .map(|grid| {
                    let mut rows: Vec<Vec<String>> =
                        grid.iter().map(|row| row.iter().map(|c| rule_normalize(c, opts)).collect()).collect();
                    let header = rows.remove(0);
                    Table { header, rows }
                })
                .collect()
        })
    }

    /// Normalise a rule's own needle with the same options as the prediction it is matched against.
    fn needle(&self, s: &str, case_sensitive: bool) -> String {
        rule_normalize(s, self.opts_for(case_sensitive))
    }
}

/// Smallest edit distance between `needle` and any substring of `hay` (Sellers' algorithm), stopping
/// early once it is known to be `<= max`. Returns whether it is `<= max`.
fn approx_contains(needle: &[char], hay: &[char], max: usize) -> bool {
    let m = needle.len();
    if m <= max {
        return true;
    }
    // col[i] = best distance of needle[..i] against a substring of hay ending at the current char.
    let mut col: Vec<usize> = (0..=m).collect();
    for &h in hay {
        let mut diag = 0; // col[i-1] of the previous column; col[0] is always 0 (free start)
        for i in 1..=m {
            let up = col[i];
            let v = if needle[i - 1] == h { diag } else { 1 + diag.min(up).min(col[i - 1]) };
            diag = up;
            col[i] = v;
        }
        if col[m] <= max {
            return true;
        }
    }
    false
}

/// End positions (exclusive, in chars) of every substring of `hay` within `max` edits of `needle`.
fn approx_match_ends(needle: &[char], hay: &[char], max: usize) -> Vec<usize> {
    let m = needle.len();
    let mut ends = Vec::new();
    let mut col: Vec<usize> = (0..=m).collect();
    if col[m] <= max {
        ends.push(0);
    }
    for (j, &h) in hay.iter().enumerate() {
        let mut diag = 0;
        for i in 1..=m {
            let up = col[i];
            let v = if needle[i - 1] == h { diag } else { 1 + diag.min(up).min(col[i - 1]) };
            diag = up;
            col[i] = v;
        }
        if col[m] <= max {
            ends.push(j + 1);
        }
    }
    ends
}

/// `(earliest start, latest start)` in chars of the substrings of `hay` within `max` edits of
/// `needle`, or `None` when there is none. Starts come from matching the reversed strings.
fn approx_match_start_range(needle: &str, hay: &[char], max: usize) -> Option<(usize, usize)> {
    let rev_needle: Vec<char> = needle.chars().rev().collect();
    let rev_hay: Vec<char> = hay.iter().rev().copied().collect();
    let ends = approx_match_ends(&rev_needle, &rev_hay, max);
    let n = hay.len();
    // A match ending at `e` in the reversed text starts at `n - e` in the original.
    Some((n - *ends.iter().max()?, n - *ends.iter().min()?))
}

/// Substring test honouring a rule's `max_diffs` (exact when it is 0).
fn contains_within(hay: &str, needle: &str, max_diffs: usize) -> bool {
    if max_diffs == 0 {
        return hay.contains(needle);
    }
    let hay: Vec<char> = hay.chars().collect();
    let needle: Vec<char> = needle.chars().collect();
    approx_contains(&needle, &hay, max_diffs)
}

/// Is `sentence` (rule-normalised) present in the prediction, exactly or within
/// [`BAG_SENTENCE_MIN_SIMILARITY`]?
fn sentence_present(cache: &PredictionCache<'_>, sentence: &str, cs: bool) -> bool {
    if sentence.is_empty() {
        return false;
    }
    if cache.text(cs).contains(sentence) {
        return true;
    }
    let needle: Vec<char> = sentence.chars().collect();
    let max = ((1.0 - BAG_SENTENCE_MIN_SIMILARITY) * needle.len() as f64).floor() as usize;
    max > 0 && approx_contains(&needle, cache.chars(cs), max)
}

/// Score a prediction against a document's rules.
///
/// Every comparison happens after [`normalize`], with `case_insensitive = !rule.case_sensitive`
/// (the rule's flag wins over `opts.case_insensitive`; `strip_markdown` and `strip_punctuation`
/// come from `opts`), followed by `squeeze_punct_spaces` on both the prediction and the rule
/// text. Because `normalize` collapses whitespace runs to a single space, runs of whitespace compare
/// equal on both sides, and spaces touching punctuation do not matter at all.
///
/// A malformed rule — one missing the fields its type needs, or whose text normalises away to
/// nothing — counts as failed with the reason in [`RuleFailure::detail`].
pub fn score_rules(prediction: &str, rules: &[Rule], opts: NormalizeOptions) -> RuleScore {
    let cache = PredictionCache::new(prediction, opts);
    let mut out = RuleScore { total: rules.len() as u32, ..RuleScore::default() };
    for rule in rules {
        let verdict = check_rule(&cache, rule);
        let bucket = out.by_type.entry(rule.rule_type.as_str().to_string()).or_default();
        bucket.total += 1;
        match verdict {
            Ok(()) => {
                bucket.passed += 1;
                out.passed += 1;
            }
            Err(detail) => out.failures.push(RuleFailure {
                id: rule.id.clone(),
                rule_type: rule.rule_type.as_str().to_string(),
                detail,
            }),
        }
    }
    out.pass_rate = if out.total == 0 { 1.0 } else { f64::from(out.passed) / f64::from(out.total) };
    out
}

/// `Ok(())` when the rule passes, `Err(detail)` when it does not.
fn check_rule(cache: &PredictionCache<'_>, rule: &Rule) -> Result<(), String> {
    let cs = rule.case_sensitive;
    let k = rule.max_diffs.unwrap_or(0);
    match rule.rule_type {
        RuleType::Present | RuleType::Absent => {
            let raw = rule.text.as_deref().ok_or_else(|| "missing `text`".to_string())?;
            let needle = cache.needle(raw, cs);
            if needle.is_empty() {
                return Err(format!("`text` {} is empty after normalisation", snippet(raw)));
            }
            let found = if k == 0 {
                cache.text(cs).contains(&needle)
            } else {
                approx_contains(&needle.chars().collect::<Vec<_>>(), cache.chars(cs), k)
            };
            match (rule.rule_type, found) {
                (RuleType::Present, false) => Err(format!("not found: {}", snippet(&needle))),
                (RuleType::Absent, true) => Err(format!("present but must be absent: {}", snippet(&needle))),
                _ => Ok(()),
            }
        }
        RuleType::Order => {
            let before_raw = rule.before.as_deref().ok_or_else(|| "missing `before`".to_string())?;
            let after_raw = rule.after.as_deref().ok_or_else(|| "missing `after`".to_string())?;
            let before = cache.needle(before_raw, cs);
            let after = cache.needle(after_raw, cs);
            if before.is_empty() || after.is_empty() {
                return Err("`before` or `after` is empty after normalisation".to_string());
            }
            if k > 0 {
                // olmOCR semantics: some fuzzy match of `before` starts before some fuzzy match of
                // `after`, i.e. the earliest `before` start precedes the latest `after` start.
                let hay = cache.chars(cs);
                let Some((b_first, _)) = approx_match_start_range(&before, hay, k) else {
                    return Err(format!("`before` not found within {k} edits: {}", snippet(&before)));
                };
                let Some((_, a_last)) = approx_match_start_range(&after, hay, k) else {
                    return Err(format!("`after` not found within {k} edits: {}", snippet(&after)));
                };
                return if b_first < a_last {
                    Ok(())
                } else {
                    Err(format!("{} occurs only before {}", snippet(&after), snippet(&before)))
                };
            }
            let hay = cache.text(cs);
            let Some(b) = hay.find(&before) else {
                return Err(format!("`before` not found: {}", snippet(&before)));
            };
            // Search `after` from the end of the first `before`, not from the start: a text that
            // repeats `after` both above and below `before` still satisfies "before, then after",
            // and the two matches can never overlap.
            let tail = &hay[b + before.len()..];
            if tail.contains(&after) {
                Ok(())
            } else if hay.contains(&after) {
                Err(format!("{} occurs only before {}", snippet(&after), snippet(&before)))
            } else {
                Err(format!("`after` not found: {}", snippet(&after)))
            }
        }
        RuleType::BagOfSentences => {
            if rule.sentences.is_empty() {
                return Err("missing `sentences`".to_string());
            }
            let threshold = bag_threshold(rule.threshold);
            let hits = rule.sentences.iter().filter(|s| sentence_present(cache, &cache.needle(s, cs), cs)).count();
            let fraction = hits as f64 / rule.sentences.len() as f64;
            // `>=` with a small epsilon: 2/3 must not fail a 0.666666… threshold.
            if fraction + 1e-9 >= threshold {
                Ok(())
            } else {
                Err(format!(
                    "{hits}/{} sentences present ({fraction:.3} < threshold {threshold:.3})",
                    rule.sentences.len()
                ))
            }
        }
        RuleType::TableCell => {
            let cell = rule.cell.as_ref().ok_or_else(|| "missing `cell`".to_string())?;
            check_table_cell(cache, cell, cs, k)
        }
    }
}

/// `table_cell`: find a matching row in any table of the prediction and compare the cell.
/// `k` (`max_diffs`) edits are tolerated in header matches and in the value.
fn check_table_cell(cache: &PredictionCache<'_>, cell: &CellRule, cs: bool, k: usize) -> Result<(), String> {
    let value = cache.needle(&cell.value, cs);
    if value.is_empty() {
        return Err("`cell.value` is empty after normalisation".to_string());
    }
    let row_header = cell.row_header.as_deref().map(|h| cache.needle(h, cs));
    if matches!(&row_header, Some(h) if h.is_empty()) {
        return Err("`cell.row_header` is empty after normalisation".to_string());
    }
    let col_header = cell.col_header.as_deref().map(|h| cache.needle(h, cs));
    if matches!(&col_header, Some(h) if h.is_empty()) {
        return Err("`cell.col_header` is empty after normalisation".to_string());
    }
    let tables = cache.tables(cs);
    if tables.is_empty() {
        return Err("prediction has no table (markdown or HTML)".to_string());
    }

    let mut saw_row = false;
    let mut saw_col = false;
    let mut seen: Vec<&str> = Vec::new();
    for table in tables {
        // A column is matched on the header row; without `col_header` every cell of the row counts.
        let col = match &col_header {
            Some(h) => {
                let Some(i) = table.header.iter().position(|c| contains_within(c, h, k)) else { continue };
                saw_col = true;
                Some(i)
            }
            None => None,
        };
        for row in &table.rows {
            if !row_matches(row, row_header.as_deref(), k) {
                continue;
            }
            saw_row = true;
            match col {
                Some(i) => {
                    let Some(got) = row.get(i) else { continue };
                    if values_close(got, &value, k) {
                        return Ok(());
                    }
                    seen.push(got);
                }
                // No `col_header`: the value may sit in any cell of the row.
                None => {
                    if row.iter().any(|c| values_close(c, &value, k)) || contains_within(&row.join(" "), &value, k) {
                        return Ok(());
                    }
                }
            }
        }
    }
    if col_header.is_some() && !saw_col {
        return Err(format!("no column matching {}", snippet(col_header.as_deref().unwrap_or(""))));
    }
    if !saw_row {
        return match &row_header {
            Some(h) => Err(format!("no row matching {}", snippet(h))),
            None => Err("prediction has no table rows".to_string()),
        };
    }
    match seen.first() {
        Some(got) => Err(format!("cell is {} , expected {}", snippet(got), snippet(&value))),
        None => Err(format!("no cell in the matched row holds {}", snippet(&value))),
    }
}

/// A row matches when its first cell contains the header; any cell is accepted as a fallback (and
/// when the rule gives no `row_header` at all).
fn row_matches(row: &[String], row_header: Option<&str>, k: usize) -> bool {
    match row_header {
        None => true,
        Some(h) => row.iter().any(|c| contains_within(c, h, k)),
    }
}

/// [`values_equal`], or within `k` Levenshtein edits when the rule allows it.
fn values_close(a: &str, b: &str, k: usize) -> bool {
    values_equal(a, b) || (k > 0 && levenshtein(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>()) <= k)
}

/// Normalised equality, with a relative tolerance of 1e-6 when both sides read as numbers.
fn values_equal(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (as_number(a), as_number(b)) {
        (Some(x), Some(y)) => (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1.0),
        _ => false,
    }
}

/// Parse a cell as a number, ignoring thousands separators, currency and percent signs.
fn as_number(s: &str) -> Option<f64> {
    let cleaned: String = s.chars().filter(|c| !matches!(c, ',' | '$' | '%' | ' ' | '\u{a0}')).collect();
    if cleaned.is_empty() {
        return None;
    }
    // Accounting negatives: `(1,234)` means `-1234`.
    let cleaned = match cleaned.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        Some(inner) => format!("-{inner}"),
        None => cleaned,
    };
    let n: f64 = cleaned.parse().ok()?;
    n.is_finite().then_some(n)
}

/// `bag_of_sentences`: the fraction of sentences that must be present.
fn bag_threshold(rule_threshold: Option<f64>) -> f64 {
    rule_threshold.unwrap_or(BAG_DEFAULT_THRESHOLD)
}

/// Shorten a string for an error message, on a character boundary.
fn snippet(s: &str) -> String {
    const MAX: usize = 60;
    if s.chars().count() <= MAX {
        format!("{s:?}")
    } else {
        let head: String = s.chars().take(MAX).collect();
        format!("{head:?}…")
    }
}

/// Turn a rule score into [`Metrics`] so rule documents aggregate with transcript documents.
///
/// `char_similarity` is the pass rate, which keeps `overall` (`100 × mean(char_similarity)`) the
/// headline number; `cer` / `wer` are its complement; `order_score` and `table_score` come from the
/// `order` and `table_cell` buckets when the document has any.
pub fn metrics_from_rules(score: &RuleScore) -> Metrics {
    let r = score.pass_rate;
    Metrics {
        char_similarity: r,
        cer: 1.0 - r,
        wer: 1.0 - r,
        word_recall: r,
        word_precision: r,
        word_f1: r,
        order_score: score.by_type.get(RuleType::Order.as_str()).and_then(RuleTypeScore::rate),
        table_score: score.by_type.get(RuleType::TableCell.as_str()).and_then(RuleTypeScore::rate),
        teds_grid: None,
        pred_chars: 0,
        truth_chars: 0,
        rule_pass_rate: Some(r),
        rules_passed: Some(score.passed),
        rules_total: Some(score.total),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basic() {
        let a: Vec<char> = "kitten".chars().collect();
        let b: Vec<char> = "sitting".chars().collect();
        assert_eq!(levenshtein(&a, &b), 3);
        assert_eq!(levenshtein::<char>(&[], &[]), 0);
        assert_eq!(levenshtein(&a, &[]), 6);
    }

    #[test]
    fn normalize_collapses() {
        let n = normalize("# Hello   **World**\n\n| a | b |\n|---|---|", NormalizeOptions::default());
        assert_eq!(n, "hello world a b");
    }

    #[test]
    fn perfect_match() {
        let m = score("Hello world", "hello  world", NormalizeOptions::default());
        assert_eq!(m.char_similarity, 1.0);
        assert_eq!(m.cer, 0.0);
        assert_eq!(m.wer, 0.0);
        assert_eq!(m.word_f1, 1.0);
    }

    #[test]
    fn partial_match() {
        let m = score("hello there world", "hello world", NormalizeOptions::default());
        assert!(m.char_similarity < 1.0 && m.char_similarity > 0.5);
        assert!((m.word_recall - 1.0).abs() < 1e-9);
        assert!((m.word_precision - 2.0 / 3.0).abs() < 1e-9);
        assert!((m.wer - 0.5).abs() < 1e-9);
    }

    #[test]
    fn order_and_table_scores() {
        let truth = "first line here\nsecond line here\nthird line here\n| a | b |\n|---|---|\n| 1 | 2 |";
        let same = score(truth, truth, NormalizeOptions::default());
        assert_eq!(same.order_score, Some(1.0));
        assert_eq!(same.table_score, Some(1.0));
        let reversed = "third line here\nsecond line here\nfirst line here\n| a | b |\n|---|---|\n| 1 | 9 |";
        let r = score(reversed, truth, NormalizeOptions::default());
        assert_eq!(r.order_score, Some(0.0));
        assert!(r.table_score.unwrap() < 1.0);
        assert_eq!(score("x", "no tables", NormalizeOptions::default()).table_score, None);
    }

    #[test]
    fn summary_counts_failures() {
        let m = score("a b c", "a b c", NormalizeOptions::default());
        let s = summarize(&[Some(m), None]);
        assert_eq!(s.documents, 2);
        assert_eq!(s.failed, 1);
        assert!((s.overall - 50.0).abs() < 1e-9);
        assert!((s.cer - 0.5).abs() < 1e-9);
    }

    // ---- rule scoring ---------------------------------------------------------------------------

    fn rule(id: &str, rule_type: RuleType) -> Rule {
        Rule {
            id: id.to_string(),
            rule_type,
            text: None,
            before: None,
            after: None,
            cell: None,
            sentences: Vec::new(),
            threshold: None,
            case_sensitive: false,
            max_diffs: None,
            source: None,
        }
    }

    fn present(id: &str, text: &str) -> Rule {
        Rule { text: Some(text.to_string()), ..rule(id, RuleType::Present) }
    }

    const TABLE_DOC: &str = "Quarterly results\n\n| Item | Q1 | Q2 |\n|---|---:|---|\n| Revenue | $1,234.00 | 2,000 |\n| Costs | 500 | 600 |\n\nEnd.";

    #[test]
    fn rule_schema_deserialises() {
        let raw = r#"[
            {"id": "a", "type": "present", "text": "x", "case_sensitive": false, "source": "up_a",
             "future_key": 7},
            {"id": "b", "type": "absent", "text": "y", "case_sensitive": true},
            {"id": "c", "type": "order", "before": "x", "after": "y", "case_sensitive": false},
            {"id": "d", "type": "table_cell",
             "cell": {"row_header": "r", "col_header": "c", "value": "1"}, "case_sensitive": false},
            {"id": "e", "type": "bag_of_sentences", "sentences": ["s"], "threshold": 0.5,
             "case_sensitive": false}
        ]"#;
        let rules: Vec<Rule> = serde_json::from_str(raw).unwrap();
        assert_eq!(rules.len(), 5);
        assert_eq!(rules[0].rule_type, RuleType::Present);
        assert_eq!(rules[0].source.as_deref(), Some("up_a"));
        assert!(!rules[0].case_sensitive);
        assert!(rules[1].case_sensitive);
        assert_eq!(rules[2].rule_type, RuleType::Order);
        assert_eq!(rules[3].cell.as_ref().unwrap().value, "1");
        assert_eq!(rules[4].rule_type, RuleType::BagOfSentences);
        assert_eq!(rules[4].threshold, Some(0.5));
        // `type` is the wire name; the enum round-trips in snake_case.
        let back = serde_json::to_value(&rules[3]).unwrap();
        assert_eq!(back["type"], "table_cell");
        assert_eq!(back["id"], "d");
        assert!(back.get("text").is_none(), "None fields are skipped: {back}");
    }

    #[test]
    fn rules_present_and_absent() {
        let doc = "# Invoice 42\n\nTotal due:   **1,024.50** USD\n";
        let rules = vec![
            present("p_ok", "invoice 42"),
            present("p_ws", "total due: 1,024.50 usd"), // whitespace runs compare equal
            present("p_bad", "invoice 43"),
            Rule { text: Some("refund".into()), ..rule("a_ok", RuleType::Absent) },
            Rule { text: Some("Invoice".into()), ..rule("a_bad", RuleType::Absent) },
        ];
        let s = score_rules(doc, &rules, NormalizeOptions::default());
        assert_eq!((s.passed, s.total), (3, 5));
        assert!((s.pass_rate - 0.6).abs() < 1e-12);
        assert_eq!(s.by_type["present"], RuleTypeScore { passed: 2, total: 3 });
        assert_eq!(s.by_type["absent"], RuleTypeScore { passed: 1, total: 2 });
        let ids: Vec<&str> = s.failures.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["p_bad", "a_bad"]);
        assert!(s.failures[0].detail.contains("not found"), "{}", s.failures[0].detail);
        assert!(s.failures[1].detail.contains("must be absent"), "{}", s.failures[1].detail);
        assert_eq!(s.failures[0].rule_type, "present");
    }

    #[test]
    fn rules_case_sensitivity() {
        let doc = "Total Amount";
        let sensitive = Rule { case_sensitive: true, ..present("cs", "total amount") };
        let insensitive = present("ci", "total amount");
        // The rule's flag wins over the run's options, in both directions.
        for opts in
            [NormalizeOptions::default(), NormalizeOptions { case_insensitive: false, ..NormalizeOptions::default() }]
        {
            assert_eq!(score_rules(doc, std::slice::from_ref(&sensitive), opts).passed, 0);
            assert_eq!(score_rules(doc, std::slice::from_ref(&insensitive), opts).passed, 1);
        }
        let exact = Rule { case_sensitive: true, ..present("cs2", "Total Amount") };
        assert_eq!(score_rules(doc, &[exact], NormalizeOptions::default()).passed, 1);
    }

    #[test]
    fn rules_order() {
        let doc = "alpha beta gamma";
        let ok = Rule { before: Some("alpha".into()), after: Some("gamma".into()), ..rule("o1", RuleType::Order) };
        let bad = Rule { before: Some("gamma".into()), after: Some("alpha".into()), ..rule("o2", RuleType::Order) };
        let missing = Rule { before: Some("alpha".into()), after: Some("delta".into()), ..rule("o3", RuleType::Order) };
        let s = score_rules(doc, &[ok, bad, missing], NormalizeOptions::default());
        assert_eq!((s.passed, s.total), (1, 3));
        assert_eq!(s.by_type["order"], RuleTypeScore { passed: 1, total: 3 });
        assert!(s.failures[0].detail.contains("occurs only before"), "{}", s.failures[0].detail);
        assert!(s.failures[1].detail.contains("`after` not found"), "{}", s.failures[1].detail);
        // Repeats: `after` occurring earlier too must not break the rule.
        let repeated = "gamma alpha beta gamma";
        let r = Rule { before: Some("alpha".into()), after: Some("gamma".into()), ..rule("o4", RuleType::Order) };
        assert_eq!(score_rules(repeated, &[r], NormalizeOptions::default()).passed, 1);
        // Overlapping needles: `after` is searched past the end of `before`'s first hit.
        let overlap = "aba";
        let o = Rule { before: Some("ab".into()), after: Some("b".into()), ..rule("o5", RuleType::Order) };
        assert_eq!(score_rules(overlap, &[o], NormalizeOptions::default()).passed, 0);
    }

    #[test]
    fn rules_bag_of_sentences() {
        let doc = "one fish. two fish.";
        let all =
            Rule { sentences: vec!["one fish".into(), "two fish".into()], ..rule("b1", RuleType::BagOfSentences) };
        assert_eq!(score_rules(doc, std::slice::from_ref(&all), NormalizeOptions::default()).passed, 1);
        // Default threshold is BAG_DEFAULT_THRESHOLD (0.8), so one miss out of two fails.
        let partial =
            Rule { sentences: vec!["one fish".into(), "red fish".into()], ..rule("b2", RuleType::BagOfSentences) };
        let s = score_rules(doc, std::slice::from_ref(&partial), NormalizeOptions::default());
        assert_eq!(s.passed, 0);
        assert!(s.failures[0].detail.contains("1/2 sentences present"), "{}", s.failures[0].detail);
        assert!(s.failures[0].detail.contains("0.500"), "{}", s.failures[0].detail);
        // …but passes below its own threshold.
        let lenient = Rule { threshold: Some(0.5), ..partial };
        assert_eq!(score_rules(doc, &[lenient], NormalizeOptions::default()).passed, 1);
        // Exact-fraction thresholds are not tripped by float error.
        let thirds = Rule {
            sentences: vec!["one fish".into(), "two fish".into(), "red fish".into()],
            threshold: Some(2.0 / 3.0),
            ..rule("b3", RuleType::BagOfSentences)
        };
        assert_eq!(score_rules(doc, &[thirds], NormalizeOptions::default()).passed, 1);
    }

    #[test]
    fn rules_table_cell() {
        let cell = |row: Option<&str>, col: Option<&str>, value: &str| CellRule {
            row_header: row.map(str::to_string),
            col_header: col.map(str::to_string),
            value: value.to_string(),
        };
        let tc = |id: &str, c: CellRule| Rule { cell: Some(c), ..rule(id, RuleType::TableCell) };
        let rules = vec![
            tc("t_exact", cell(Some("Costs"), Some("Q2"), "600")),
            // Numeric tolerance: `,` `$` `%` are stripped before the comparison.
            tc("t_num", cell(Some("revenue"), Some("q1"), "1234")),
            tc("t_num2", cell(Some("revenue"), Some("q2"), "2000.0000001")),
            // No column header: the value may be in any cell of the row.
            tc("t_anycol", cell(Some("revenue"), None, "2,000")),
            // No row header: any row may hold it.
            tc("t_anyrow", cell(None, Some("Q1"), "500")),
            tc("t_wrong", cell(Some("Costs"), Some("Q2"), "601")),
            tc("t_nocol", cell(Some("Costs"), Some("Q3"), "600")),
            tc("t_norow", cell(Some("Profit"), Some("Q2"), "600")),
        ];
        let s = score_rules(TABLE_DOC, &rules, NormalizeOptions::default());
        assert_eq!(s.by_type["table_cell"], RuleTypeScore { passed: 5, total: 8 });
        let fail = |id: &str| s.failures.iter().find(|f| f.id == id).map(|f| f.detail.as_str()).unwrap_or("");
        assert!(fail("t_wrong").contains("cell is"), "{}", fail("t_wrong"));
        assert!(fail("t_nocol").contains("no column matching"), "{}", fail("t_nocol"));
        assert!(fail("t_norow").contains("no row matching"), "{}", fail("t_norow"));
        // No table at all.
        let none =
            score_rules("just prose", &[tc("t_none", cell(Some("a"), Some("b"), "1"))], NormalizeOptions::default());
        assert_eq!(none.passed, 0);
        assert!(none.failures[0].detail.contains("prediction has no table"), "{}", none.failures[0].detail);
    }

    #[test]
    fn rules_table_cell_escapes_and_case() {
        let doc = "| Name | Note |\n| --- | --- |\n| A \\| B | Yes |";
        let c = CellRule { row_header: Some("a | b".into()), col_header: Some("note".into()), value: "yes".into() };
        let r = Rule { cell: Some(c.clone()), ..rule("esc", RuleType::TableCell) };
        assert_eq!(score_rules(doc, &[r], NormalizeOptions::default()).passed, 1);
        let strict = Rule { cell: Some(c), case_sensitive: true, ..rule("esc_cs", RuleType::TableCell) };
        assert_eq!(score_rules(doc, &[strict], NormalizeOptions::default()).passed, 0);
    }

    #[test]
    fn rules_malformed_count_as_failures() {
        let rules = vec![
            rule("m_text", RuleType::Present),
            Rule { after: Some("x".into()), ..rule("m_before", RuleType::Order) },
            Rule { before: Some("x".into()), ..rule("m_after", RuleType::Order) },
            rule("m_cell", RuleType::TableCell),
            rule("m_sent", RuleType::BagOfSentences),
            Rule { text: Some("**".into()), ..rule("m_empty", RuleType::Present) },
        ];
        let s = score_rules("anything at all", &rules, NormalizeOptions::default());
        assert_eq!((s.passed, s.total), (0, 6));
        assert_eq!(s.pass_rate, 0.0);
        let detail = |id: &str| s.failures.iter().find(|f| f.id == id).map(|f| f.detail.clone()).unwrap();
        assert!(detail("m_text").contains("missing `text`"));
        assert!(detail("m_before").contains("missing `before`"));
        assert!(detail("m_after").contains("missing `after`"));
        assert!(detail("m_cell").contains("missing `cell`"));
        assert!(detail("m_sent").contains("missing `sentences`"));
        assert!(detail("m_empty").contains("empty after normalisation"), "{}", detail("m_empty"));
    }

    #[test]
    fn rules_empty_set_is_vacuously_true() {
        let s = score_rules("anything", &[], NormalizeOptions::default());
        assert_eq!((s.passed, s.total), (0, 0));
        assert_eq!(s.pass_rate, 1.0);
        assert!(s.by_type.is_empty());
    }

    #[test]
    fn metrics_from_rules_fill_headline_fields() {
        let rules = vec![
            present("p1", "alpha"),
            present("p2", "nope"),
            Rule { before: Some("alpha".into()), after: Some("beta".into()), ..rule("o1", RuleType::Order) },
            Rule {
                cell: Some(CellRule { row_header: None, col_header: None, value: "zzz".into() }),
                ..rule("t1", RuleType::TableCell)
            },
        ];
        let s = score_rules("alpha beta", &rules, NormalizeOptions::default());
        assert_eq!((s.passed, s.total), (2, 4));
        let m = metrics_from_rules(&s);
        assert_eq!(m.char_similarity, 0.5);
        assert_eq!(m.cer, 0.5);
        assert_eq!(m.wer, 0.5);
        assert_eq!(m.word_f1, 0.5);
        assert_eq!(m.rule_pass_rate, Some(0.5));
        assert_eq!(m.rules_passed, Some(2));
        assert_eq!(m.rules_total, Some(4));
        assert_eq!(m.order_score, Some(1.0));
        assert_eq!(m.table_score, Some(0.0));
        // No rules of a type -> no bucket, no metric.
        let only_present = score_rules("alpha", &[present("p", "alpha")], NormalizeOptions::default());
        let m2 = metrics_from_rules(&only_present);
        assert_eq!(m2.order_score, None);
        assert_eq!(m2.table_score, None);
        // Rule metrics aggregate through `summarize`.
        let s2 = summarize(&[Some(m), Some(m2), None]);
        assert_eq!(s2.documents, 3);
        assert_eq!(s2.failed, 1);
        assert!((s2.rule_pass_rate.unwrap() - 0.75).abs() < 1e-12);
        assert!((s2.overall - 50.0).abs() < 1e-9);
        // A transcript document contributes no rule fields, and its JSON keeps the old shape.
        let plain = score("a b c", "a b c", NormalizeOptions::default());
        assert_eq!(plain.rule_pass_rate, None);
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("rule_pass_rate"), "{json}");
        assert_eq!(summarize(&[Some(plain)]).rule_pass_rate, None);
    }

    #[test]
    fn headline_prefers_table_score_for_table_only_docs() {
        let m = Metrics { char_similarity: 0.311, table_score: Some(0.99), ..Metrics::default() };
        assert_eq!(headline(&m, false), 0.311);
        assert_eq!(headline(&m, true), 0.99);
        // No table in the prediction: fall back to char_similarity rather than inventing a score.
        let no_table = Metrics { char_similarity: 0.4, table_score: None, ..Metrics::default() };
        assert_eq!(headline(&no_table, true), 0.4);

        let docs = [Some(m), Some(no_table), None];
        let s = summarize_with(&docs, &[true, true, true]);
        // The headline lives in `score`/`overall`; `char_similarity` stays the literal mean.
        assert!((s.headline - (0.99 + 0.4) / 3.0).abs() < 1e-12);
        assert!((s.overall - 100.0 * (0.99 + 0.4) / 3.0).abs() < 1e-9);
        assert!((s.char_similarity - (0.311 + 0.4) / 3.0).abs() < 1e-12);
        assert_eq!(s.documents, 3);
        assert_eq!(s.failed, 1);
        // Backward compatible: no flags (or all false) is exactly `summarize`.
        assert_eq!(summarize_with(&docs, &[]), summarize(&docs));
        assert_eq!(summarize_with(&docs, &[false, false, false]), summarize(&docs));
        // A short flag slice means "not table-only" for the documents it does not cover.
        assert_eq!(summarize_with(&docs, &[false]), summarize(&docs));
    }

    // A committed ParseBench rules file, copied verbatim into `tests/fixtures/` so the crate stays
    // self-contained when packaged (source: benchmark/datasets/parsebench/rules/text_sparse_note.json).
    const SPARSE_NOTE_RULES: &str = include_str!("../tests/fixtures/rules_text_sparse_note.json");

    #[test]
    fn parsebench_rules_file_scores_end_to_end() {
        let rules: Vec<Rule> = serde_json::from_str(SPARSE_NOTE_RULES).unwrap();
        assert_eq!(rules.len(), 4);
        assert_eq!(rules.iter().filter(|r| r.rule_type == RuleType::Present).count(), 3);
        assert_eq!(rules.iter().filter(|r| r.rule_type == RuleType::BagOfSentences).count(), 1);
        assert!(rules.iter().all(|r| r.source.is_some() && !r.id.is_empty()));

        // The document's own strings, joined: everything the rules assert is present.
        let mut doc = String::new();
        for r in &rules {
            if let Some(t) = &r.text {
                doc.push_str(t);
                doc.push('\n');
            }
            for s in &r.sentences {
                doc.push_str(s);
                doc.push('\n');
            }
        }
        let perfect = score_rules(&doc, &rules, NormalizeOptions::default());
        assert_eq!(perfect.pass_rate, 1.0, "failures: {:?}", perfect.failures);
        assert_eq!(perfect.passed, perfect.total);
        assert!(perfect.failures.is_empty());
        assert_eq!(metrics_from_rules(&perfect).char_similarity, 1.0);

        let empty = score_rules("", &rules, NormalizeOptions::default());
        assert_eq!(empty.pass_rate, 0.0);
        assert_eq!(empty.passed, 0);
        assert_eq!(empty.failures.len(), rules.len());
        assert_eq!(metrics_from_rules(&empty).cer, 1.0);
    }

    #[test]
    fn parsebench_rules_dataset_loads() {
        // Runs against the real dataset when the repository is present (it is not shipped in the
        // published crate, so a missing directory just skips).
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmark/datasets/parsebench/rules");
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        let mut files = 0;
        let mut total = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).unwrap();
            let rules: Vec<Rule> = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(!rules.is_empty(), "{} has no rules", path.display());
            files += 1;
            total += rules.len();
            // Every `present` assertion holds for a document made of the assertions themselves.
            let doc: String = rules.iter().filter_map(|r| r.text.as_deref()).collect::<Vec<_>>().join("\n");
            let s = score_rules(&doc, &rules, NormalizeOptions::default());
            assert_eq!(
                s.by_type.get("present").and_then(RuleTypeScore::rate),
                Some(1.0),
                "{}: {:?}",
                path.display(),
                s.failures.iter().take(3).collect::<Vec<_>>()
            );
            assert_eq!(score_rules("", &rules, NormalizeOptions::default()).pass_rate, 0.0);
        }
        assert!(files >= 15, "expected the committed parsebench rules files, found {files}");
        assert!(total > 3_000, "expected thousands of rules, found {total}");
    }

    // ---- scorer v2 ------------------------------------------------------------------------------

    const HTML_TABLE_DOC: &str =
        "Quarterly results\n\n<table><thead><tr><th>Item</th><th>Q1</th><th>Q2</th></tr></thead>\
        <tbody><tr><td><b>Revenue</b></td><td>$1,234.00</td><td>2,000</td></tr>\
        <tr><td>Costs</td><td>500</td><td>600</td></tr></tbody></table>\n\nEnd.";

    #[test]
    fn html_table_scores_like_the_equivalent_markdown_table() {
        let truth = "| Item | Q1 | Q2 |\n|---|---|---|\n| Revenue | $1,234.00 | 2,000 |\n| Costs | 500 | 600 |";
        let md = score(TABLE_DOC, truth, NormalizeOptions::default());
        let html = score(HTML_TABLE_DOC, truth, NormalizeOptions::default());
        assert_eq!(md.table_score, Some(1.0));
        assert_eq!(html.table_score, Some(1.0), "an HTML table used to score 0 here");
        assert_eq!(md.teds_grid, Some(1.0));
        assert_eq!(html.teds_grid, Some(1.0));
        // A wrong cell costs both metrics something.
        let wrong = HTML_TABLE_DOC.replace("600", "900");
        let w = score(&wrong, truth, NormalizeOptions::default());
        assert!(w.table_score.unwrap() < 1.0 && w.teds_grid.unwrap() < 1.0);
        // Headline for a table-only doc follows.
        assert_eq!(headline(&html, true), 1.0);
    }

    /// The real Reducto `r-1` output for ParseBench `637951191e7b…_page1` (an HTML table with
    /// `colspan`/`rowspan`), scored against the committed truth. Scorer v1 gave it `table_score` 0.
    #[test]
    fn real_reducto_r1_html_output_scores_against_parsebench_truth() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let pred = root.join(
            "benchmark/results/outputs/run-20260911T111039Z/reducto_r-1/parsebench/637951191e7b35ae07dbd4c76a2e6690c370_pg2_pg1_page1.md",
        );
        let truth =
            root.join("benchmark/datasets/parsebench/truth/637951191e7b35ae07dbd4c76a2e6690c370_pg2_pg1_page1.md");
        let (Ok(pred), Ok(truth)) = (std::fs::read_to_string(pred), std::fs::read_to_string(truth)) else {
            return; // not in a packaged crate
        };
        let m = score(&pred, &truth, NormalizeOptions::default());
        let t = m.table_score.expect("truth has a table");
        assert!(t > 0.6, "HTML table must be read: table_score {t}");
        let teds = m.teds_grid.expect("truth has a table");
        // The content is right but the structure is not the truth's: Reducto splits `24 d` into two
        // columns and `6.4 oz wt/a` into two, so TEDS is well below the flat similarity.
        assert!(teds > 0.3 && teds < t, "teds_grid {teds} vs table_score {t}");
    }

    #[test]
    fn table_cell_rules_read_html_tables() {
        let cell = |row: &str, col: &str, value: &str| Rule {
            cell: Some(CellRule { row_header: Some(row.into()), col_header: Some(col.into()), value: value.into() }),
            ..rule("h", RuleType::TableCell)
        };
        let rules = [cell("Costs", "Q2", "600"), cell("revenue", "q1", "1234"), cell("Costs", "Q2", "601")];
        let s = score_rules(HTML_TABLE_DOC, &rules, NormalizeOptions::default());
        assert_eq!(s.by_type["table_cell"], RuleTypeScore { passed: 2, total: 3 });
        // Spanned cells repeat, so a rowspan'd row header still labels the second row.
        let spans = "<table><tr><th>Region</th><th>Year</th><th>Sales</th></tr>\
            <tr><td rowspan=\"2\">North</td><td>2023</td><td>10</td></tr><tr><td>2024</td><td>12</td></tr></table>";
        let r = Rule {
            cell: Some(CellRule {
                row_header: Some("north".into()),
                col_header: Some("sales".into()),
                value: "12".into(),
            }),
            ..rule("span", RuleType::TableCell)
        };
        assert_eq!(score_rules(spans, &[r], NormalizeOptions::default()).passed, 1);
    }

    #[test]
    fn punctuation_spacing_is_insignificant_for_rules() {
        // ParseBench-style tokenised rule text against the page as printed, and the reverse.
        let doc = "THIS ASSET PURCHASE AGREEMENT (this \"Agreement\"), dated as of [June 23], 2020";
        let tokenised =
            present("tok", "THIS ASSET PURCHASE AGREEMENT (this \" Agreement \"), dated as of [ June 23 ] , 2020");
        let s = score_rules(doc, &[tokenised], NormalizeOptions::default());
        assert_eq!(s.passed, 1, "{:?}", s.failures);
        let spaced_doc = "Total : $ 1,024 . 50 ( net )";
        assert_eq!(
            score_rules(spaced_doc, &[present("rev", "total: $1,024.50 (net)")], NormalizeOptions::default()).passed,
            1
        );
        // Order rules get the same treatment.
        let o = Rule {
            before: Some("(this \" Agreement \")".into()),
            after: Some("[ June 23 ]".into()),
            ..rule("o", RuleType::Order)
        };
        assert_eq!(score_rules(doc, &[o], NormalizeOptions::default()).passed, 1);
        // Words still need their spaces: this is not "ignore all whitespace".
        assert_eq!(score_rules(doc, &[present("ws", "thisagreement")], NormalizeOptions::default()).passed, 0);
        assert_eq!(squeeze_punct_spaces("a ( b ) c , d - e"), "a(b)c,d-e");
        assert_eq!(squeeze_punct_spaces("plain words stay"), "plain words stay");
        // Transcript metrics are untouched by it.
        assert!(score("a ( b )", "a (b)", NormalizeOptions::default()).char_similarity < 1.0);
    }

    #[test]
    fn bag_of_sentences_is_fuzzy_per_sentence_with_a_fraction_threshold() {
        let doc = "The quick brown fox jumps over the lazy dog. Pack my box with five dozen liquor jugs. \
                   How vexingly quick daft zebras jump.";
        let bag = |sentences: &[&str], threshold: Option<f64>| Rule {
            sentences: sentences.iter().map(|s| s.to_string()).collect(),
            threshold,
            ..rule("bag", RuleType::BagOfSentences)
        };
        // One OCR slip per sentence (≥ 0.8 similar) still counts as present.
        let slipped =
            bag(&["The quick brown f0x jumps over the lazy dog", "Pack my b0x with five dozen liquor jugs"], Some(1.0));
        assert_eq!(score_rules(doc, &[slipped], NormalizeOptions::default()).passed, 1);
        // A different sentence does not.
        let unrelated = bag(&["Sphinx of black quartz, judge my vow"], Some(1.0));
        assert_eq!(score_rules(doc, &[unrelated], NormalizeOptions::default()).passed, 0);
        // Default threshold 0.8: 4 of 5 present passes, 3 of 5 fails.
        let five = [
            "The quick brown fox jumps over the lazy dog",
            "Pack my box with five dozen liquor jugs",
            "How vexingly quick daft zebras jump",
            "Sphinx of black quartz, judge my vow",
            "Jackdaws love my big sphinx of quartz",
        ];
        assert_eq!(bag_threshold(None), BAG_DEFAULT_THRESHOLD);
        let s = score_rules(doc, &[bag(&five, None)], NormalizeOptions::default());
        assert_eq!(s.passed, 0);
        assert!(s.failures[0].detail.contains("3/5"), "{}", s.failures[0].detail);
        let doc4 = format!("{doc} Sphinx of black quartz, judge my vow.");
        assert_eq!(score_rules(&doc4, &[bag(&five, None)], NormalizeOptions::default()).passed, 1);
        // Sellers substring distance: exact, one edit, too many edits.
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(approx_contains(&c("brown"), &c("the brown fox"), 0));
        assert!(approx_contains(&c("brwn"), &c("the brown fox"), 1));
        assert!(!approx_contains(&c("green"), &c("the brown fox"), 1));
    }

    #[test]
    fn html_entities_decode_in_normalisation() {
        let n = normalize("Pear &amp; Fig &lt;b&gt; &#233;t&eacute;", NormalizeOptions::default());
        // Tags are stripped before decoding, so an escaped `<b>` stays text; unknown names stay as written.
        assert_eq!(n, "pear & fig <b> ét&eacute;");
    }

    #[test]
    fn summary_json_is_backward_compatible() {
        // A scorer-v1 summary has no `headline`: it is recovered from `overall`.
        let old = r#"{"documents": 2, "failed": 0, "char_similarity": 0.9, "cer": 0.1, "wer": 0.1,
                      "word_f1": 0.9, "table_score": 0.8, "overall": 90.0}"#;
        let s: Summary = serde_json::from_str(old).unwrap();
        assert!((s.headline - 0.9).abs() < 1e-12);
        assert_eq!(s.teds_grid, None);
        // Round trip of a v2 summary keeps every field.
        let m = score("| a |\n|---|\n| 1 |", "| a |\n|---|\n| 1 |", NormalizeOptions::default());
        let v2 = summarize_with(&[Some(m), None], &[true, false]);
        let json = serde_json::to_string(&v2).unwrap();
        assert!(json.contains("\"headline\":0.5"), "{json}");
        assert!(json.contains("\"teds_grid\":1.0"), "{json}");
        assert_eq!(serde_json::from_str::<Summary>(&json).unwrap(), v2);
        // Old per-document metrics without `teds_grid` still load.
        let m_old: Metrics = serde_json::from_str(
            r#"{"char_similarity": 1.0, "cer": 0.0, "wer": 0.0, "word_recall": 1.0, "word_precision": 1.0,
                "word_f1": 1.0, "table_score": 1.0, "pred_chars": 3, "truth_chars": 3}"#,
        )
        .unwrap();
        assert_eq!(m_old.teds_grid, None);
    }

    #[test]
    fn max_diffs_makes_present_absent_order_and_table_cell_fuzzy() {
        let opts = NormalizeOptions::default();
        let doc = "The Treaty of Westphalia was signed in 1648 after thirty years of war.";
        let with = |r: Rule, k: usize| Rule { max_diffs: Some(k), ..r };
        // present: two OCR slips pass at max_diffs 2, not at 0 or 1.
        let p = present("p", "Treaty of Westphalla was sined");
        assert_eq!(score_rules(doc, std::slice::from_ref(&p), opts).passed, 0);
        assert_eq!(score_rules(doc, &[with(p.clone(), 1)], opts).passed, 0);
        assert_eq!(score_rules(doc, &[with(p.clone(), 2)], opts).passed, 1);
        // `max_diffs: 0` is exact, like no field at all.
        assert_eq!(score_rules(doc, &[with(p, 0)], opts).passed, 0);
        // absent: a near match counts as present, so the rule fails.
        let a = Rule { text: Some("Treaty of Westphalla".into()), ..rule("a", RuleType::Absent) };
        assert_eq!(score_rules(doc, std::slice::from_ref(&a), opts).passed, 1);
        assert_eq!(score_rules(doc, &[with(a, 1)], opts).passed, 0);
        // order: fuzzy on both sides; olmOCR semantics (some `before` start precedes some `after`).
        let o =
            Rule { before: Some("Westphalla".into()), after: Some("thirty yeers".into()), ..rule("o", RuleType::Order) };
        assert_eq!(score_rules(doc, std::slice::from_ref(&o), opts).passed, 0);
        assert_eq!(score_rules(doc, &[with(o, 1)], opts).passed, 1);
        let rev =
            Rule { before: Some("thirty yeers".into()), after: Some("Westphalla".into()), ..rule("r", RuleType::Order) };
        let s = score_rules(doc, &[with(rev, 1)], opts);
        assert_eq!(s.passed, 0);
        assert!(s.failures[0].detail.contains("occurs only before"), "{}", s.failures[0].detail);
        let missing =
            Rule { before: Some("Versailles".into()), after: Some("war".into()), ..rule("m", RuleType::Order) };
        assert!(score_rules(doc, &[with(missing, 1)], opts).failures[0].detail.contains("within 1 edits"));
        // table_cell: headers and value within max_diffs.
        let tc = Rule {
            cell: Some(CellRule {
                row_header: Some("Revenu".into()),
                col_header: Some("Q 2".into()),
                value: "2,001".into(),
            }),
            ..rule("t", RuleType::TableCell)
        };
        assert_eq!(score_rules(TABLE_DOC, std::slice::from_ref(&tc), opts).passed, 0);
        assert_eq!(score_rules(TABLE_DOC, &[with(tc, 1)], opts).passed, 1);
        // The wire field round-trips and is optional.
        let r: Rule = serde_json::from_str(r#"{"id":"x","type":"present","text":"a","max_diffs":3}"#).unwrap();
        assert_eq!(r.max_diffs, Some(3));
        assert!(serde_json::to_value(present("y", "a")).unwrap().get("max_diffs").is_none());
        // Start positions of fuzzy matches.
        let hay: Vec<char> = "xx abc yy abd".chars().collect();
        assert_eq!(approx_match_start_range("abc", &hay, 0), Some((3, 3)));
        assert_eq!(approx_match_start_range("abc", &hay, 1).map(|(a, _)| a), Some(2));
        assert!(approx_match_start_range("abc", &hay, 1).unwrap().1 >= 10);
        assert_eq!(approx_match_start_range("zzz", &hay, 0), None);
    }
}
