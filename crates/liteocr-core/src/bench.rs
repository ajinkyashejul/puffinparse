//! Benchmark metrics: text normalisation and accuracy scores between a prediction and ground truth.
//!
//! All metrics are deterministic and need no model or network access.

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

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
    let s = if opts.strip_markdown { crate::types::markdown_to_text(&s) } else { s };
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
    /// Similarity restricted to markdown table lines. `None` if the truth has no tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_score: Option<f64>,
    pub pred_chars: usize,
    pub truth_chars: usize,
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
    let table_score = table_similarity(pred, truth, opts);

    Metrics {
        char_similarity,
        cer,
        wer,
        word_recall,
        word_precision,
        word_f1,
        order_score,
        table_score,
        pred_chars: p_chars.len(),
        truth_chars: t_chars.len(),
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

/// Similarity of the markdown-table rows only. `None` if the truth has no table rows.
fn table_similarity(pred: &str, truth: &str, opts: NormalizeOptions) -> Option<f64> {
    let rows = |s: &str| -> Vec<String> {
        s.lines()
            .map(str::trim)
            .filter(|l| l.starts_with('|') && !l.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')))
            .map(|l| normalize(l, NormalizeOptions { strip_markdown: false, ..opts }))
            .map(|l| l.replace('|', " "))
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    };
    let t = rows(truth);
    if t.is_empty() {
        return None;
    }
    let p = rows(pred);
    let tj: Vec<char> = t.join("\n").chars().collect();
    let pj: Vec<char> = p.join("\n").chars().collect();
    let max_len = tj.len().max(pj.len());
    if max_len == 0 {
        return Some(1.0);
    }
    Some(1.0 - levenshtein(&pj, &tj) as f64 / max_len as f64)
}

/// Aggregate of per-document metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Summary {
    pub documents: usize,
    pub failed: usize,
    pub char_similarity: f64,
    pub cer: f64,
    pub wer: f64,
    pub word_f1: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_score: Option<f64>,
    /// `100 * mean(char_similarity)`; failures count as 0.
    pub overall: f64,
}

/// Mean over documents. `failed` documents (no metrics) count as zero accuracy.
pub fn summarize(metrics: &[Option<Metrics>]) -> Summary {
    let n = metrics.len();
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
    let char_similarity = mean(&|m| m.char_similarity);
    Summary {
        documents: n,
        failed,
        char_similarity,
        // error rates: failures count as 1.0
        cer: if n == 0 { 0.0 } else { (ok.iter().map(|m| m.cer).sum::<f64>() + failed as f64) / n as f64 },
        wer: if n == 0 { 0.0 } else { (ok.iter().map(|m| m.wer).sum::<f64>() + failed as f64) / n as f64 },
        word_f1: mean(&|m| m.word_f1),
        order_score: mean_opt(&|m| m.order_score),
        table_score: mean_opt(&|m| m.table_score),
        overall: 100.0 * char_similarity,
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
}
