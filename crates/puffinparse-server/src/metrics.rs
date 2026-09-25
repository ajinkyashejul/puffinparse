//! Prometheus text exposition, hand-rolled (a handful of counters and one histogram do not
//! justify a metrics crate).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Mutex;

/// Latency buckets in seconds: document parsing ranges from sub-second OCR to multi-minute jobs.
const BUCKETS: [f64; 11] = [0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0];

#[derive(Debug, Default, Clone)]
struct Histogram {
    counts: [u64; BUCKETS.len()],
    sum: f64,
    count: u64,
}

impl Histogram {
    fn observe(&mut self, secs: f64) {
        for (i, b) in BUCKETS.iter().enumerate() {
            if secs <= *b {
                self.counts[i] += 1;
            }
        }
        self.sum += secs;
        self.count += 1;
    }
}

#[derive(Debug, Default)]
struct Inner {
    /// (mode, model, http status) → count.
    requests: BTreeMap<(String, String, u16), u64>,
    /// error type → count.
    errors: BTreeMap<String, u64>,
    latency: BTreeMap<String, Histogram>,
    pages: BTreeMap<String, u64>,
    cost: BTreeMap<String, f64>,
    fallbacks: u64,
    /// Async job lifecycle event (`submitted`, `succeeded`, `failed`) → count.
    jobs: BTreeMap<&'static str, u64>,
}

#[derive(Debug, Default)]
pub struct Metrics {
    inner: Mutex<Inner>,
}

/// One finished request, as the metrics see it.
#[derive(Debug)]
pub struct Observation<'a> {
    pub mode: &'a str,
    /// Served model on success, the requested alias/model when it was valid, else `"-"`.
    pub model: &'a str,
    pub status: u16,
    pub error_type: Option<&'a str>,
    pub latency_secs: f64,
    pub pages: u32,
    pub cost_usd: Option<f64>,
    pub fell_back: bool,
}

impl Metrics {
    pub fn observe(&self, o: &Observation<'_>) {
        let mut m = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        *m.requests.entry((o.mode.to_string(), o.model.to_string(), o.status)).or_default() += 1;
        if let Some(t) = o.error_type {
            *m.errors.entry(t.to_string()).or_default() += 1;
        }
        m.latency.entry(o.mode.to_string()).or_default().observe(o.latency_secs);
        if o.pages > 0 {
            *m.pages.entry(o.model.to_string()).or_default() += u64::from(o.pages);
        }
        if let Some(c) = o.cost_usd {
            *m.cost.entry(o.model.to_string()).or_default() += c;
        }
        if o.fell_back {
            m.fallbacks += 1;
        }
    }

    /// Count an async job event: `submitted`, or the first time it is seen `succeeded` / `failed`.
    pub fn job_event(&self, event: &'static str) {
        let mut m = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        *m.jobs.entry(event).or_default() += 1;
    }

    pub fn render(&self) -> String {
        let m = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut out = String::new();
        header(&mut out, "puffinparse_requests_total", "counter", "Requests handled, by mode, model and HTTP status.");
        for ((mode, model, status), n) in &m.requests {
            let _ = writeln!(
                out,
                "puffinparse_requests_total{{mode=\"{}\",model=\"{}\",status=\"{status}\"}} {n}",
                esc(mode),
                esc(model)
            );
        }
        header(&mut out, "puffinparse_errors_total", "counter", "Failed requests, by error type.");
        for (kind, n) in &m.errors {
            let _ = writeln!(out, "puffinparse_errors_total{{type=\"{}\"}} {n}", esc(kind));
        }
        header(&mut out, "puffinparse_request_duration_seconds", "histogram", "End-to-end request latency.");
        for (mode, h) in &m.latency {
            let mode = esc(mode);
            for (b, c) in BUCKETS.iter().zip(h.counts.iter()) {
                let _ = writeln!(out, "puffinparse_request_duration_seconds_bucket{{mode=\"{mode}\",le=\"{b}\"}} {c}");
            }
            let _ =
                writeln!(out, "puffinparse_request_duration_seconds_bucket{{mode=\"{mode}\",le=\"+Inf\"}} {}", h.count);
            let _ = writeln!(out, "puffinparse_request_duration_seconds_sum{{mode=\"{mode}\"}} {}", h.sum);
            let _ = writeln!(out, "puffinparse_request_duration_seconds_count{{mode=\"{mode}\"}} {}", h.count);
        }
        header(&mut out, "puffinparse_pages_total", "counter", "Pages processed, by served model.");
        for (model, n) in &m.pages {
            let _ = writeln!(out, "puffinparse_pages_total{{model=\"{}\"}} {n}", esc(model));
        }
        header(&mut out, "puffinparse_cost_usd_total", "counter", "Estimated list-price cost in USD, by served model.");
        for (model, c) in &m.cost {
            let _ = writeln!(out, "puffinparse_cost_usd_total{{model=\"{}\"}} {c}", esc(model));
        }
        header(&mut out, "puffinparse_fallbacks_total", "counter", "Requests served by a fallback target.");
        let _ = writeln!(out, "puffinparse_fallbacks_total {}", m.fallbacks);
        header(
            &mut out,
            "puffinparse_jobs_total",
            "counter",
            "Async jobs submitted, and first observed succeeded or failed.",
        );
        for (event, n) in &m.jobs {
            let _ = writeln!(out, "puffinparse_jobs_total{{event=\"{event}\"}} {n}");
        }
        out
    }
}

fn header(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_counters_and_histogram() {
        let m = Metrics::default();
        m.observe(&Observation {
            mode: "parse",
            model: "reducto/standard",
            status: 200,
            error_type: None,
            latency_secs: 1.5,
            pages: 2,
            cost_usd: Some(0.03),
            fell_back: false,
        });
        m.observe(&Observation {
            mode: "parse",
            model: "we\"ird",
            status: 502,
            error_type: Some("provider_error"),
            latency_secs: 0.1,
            pages: 0,
            cost_usd: None,
            fell_back: false,
        });
        let text = m.render();
        assert!(text.contains(r#"puffinparse_requests_total{mode="parse",model="reducto/standard",status="200"} 1"#));
        assert!(text.contains(r#"model="we\"ird""#));
        assert!(text.contains(r#"puffinparse_errors_total{type="provider_error"} 1"#));
        assert!(text.contains(r#"puffinparse_request_duration_seconds_bucket{mode="parse",le="1"} 1"#));
        assert!(text.contains(r#"puffinparse_request_duration_seconds_bucket{mode="parse",le="2.5"} 2"#));
        assert!(text.contains(r#"puffinparse_request_duration_seconds_count{mode="parse"} 2"#));
        assert!(text.contains(r#"puffinparse_pages_total{model="reducto/standard"} 2"#));
        assert!(text.contains(r#"puffinparse_cost_usd_total{model="reducto/standard"} 0.03"#));
    }
}
