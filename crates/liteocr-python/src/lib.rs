//! PyO3 bindings: the `liteocr._core` extension module.
//!
//! The Python package (`python/liteocr`) wraps these functions with a typed, idiomatic API.
//! All heavy lifting stays in `liteocr-core`; this layer only converts values and bridges runtimes.
//!
//! Everything is organised around the three **modes** of the core — `parse` (layout-aware
//! markdown + blocks), `ocr` (plain text + word/line boxes) and `extract` (JSON schema →
//! structured data). A model can only serve the modes it declares, so each entry point below
//! resolves the model string against its own mode.

use liteocr_core::compat::Format;
use liteocr_core::router::{Router as CoreRouter, RouterConfig, Strategy};
use liteocr_core::{
    DocumentInput, DocumentRequest, ExtractRequest, ExtractResponse, JobHandle, JobStatus, Mode, ParseResponse,
    RetrieveOptions,
};
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use pythonize::{depythonize, pythonize};
use serde::Serialize;
use std::sync::Arc;

pyo3::create_exception!(_core, CoreError, PyException, "Raised by the core; args[0] is a JSON-encoded error.");

fn to_py_err(e: liteocr_core::Error) -> PyErr {
    CoreError::new_err(serde_json::to_string(&e).unwrap_or_else(|_| e.to_string()))
}

/// Parse a mode string (`"parse" | "ocr" | "extract"`), surfacing a typed core error.
fn parse_mode(mode: &str) -> PyResult<Mode> {
    mode.parse::<Mode>().map_err(to_py_err)
}

/// Replace the document input with the separately-passed bytes, keeping the declared filename.
fn attach_bytes(input: &mut DocumentInput, bytes: &Bound<'_, PyBytes>) {
    let filename = match &input {
        DocumentInput::Bytes { filename, .. } => filename.clone(),
        other => other.filename(),
    };
    *input = DocumentInput::Bytes { data: bytes::Bytes::copy_from_slice(bytes.as_bytes()), filename };
}

fn build_request(request: &Bound<'_, PyDict>, data: Option<&Bound<'_, PyBytes>>) -> PyResult<DocumentRequest> {
    let mut req: DocumentRequest = depythonize(request.as_any())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("invalid request: {e}")))?;
    if let Some(bytes) = data {
        attach_bytes(&mut req.input, bytes);
    }
    Ok(req)
}

/// Build an [`ExtractRequest`] from a flattened dict (document fields plus `schema`,
/// `instructions`, `citations`).
fn build_extract_request(request: &Bound<'_, PyDict>, data: Option<&Bound<'_, PyBytes>>) -> PyResult<ExtractRequest> {
    let mut req: ExtractRequest = depythonize(request.as_any())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("invalid extract request: {e}")))?;
    if let Some(bytes) = data {
        attach_bytes(&mut req.document.input, bytes);
    }
    Ok(req)
}

fn to_py<T: Serialize>(py: Python<'_>, value: &T) -> PyResult<Py<PyAny>> {
    Ok(pythonize(py, value)?.unbind())
}

// ---- parse mode ----------------------------------------------------------------------------------

/// Run a `parse`-mode request synchronously. `request` is a dict matching `DocumentRequest`; `data`
/// optionally supplies the document bytes (avoids base64-encoding large files through the dict).
#[pyfunction]
#[pyo3(signature = (request, data=None))]
fn parse(py: Python<'_>, request: &Bound<'_, PyDict>, data: Option<&Bound<'_, PyBytes>>) -> PyResult<Py<PyAny>> {
    let req = build_request(request, data)?;
    let rt = pyo3_async_runtimes::tokio::get_runtime();
    let resp = py.detach(|| rt.block_on(liteocr_core::parse(req))).map_err(to_py_err)?;
    to_py(py, &resp)
}

/// Async variant of [`parse`] returning an awaitable.
#[pyfunction]
#[pyo3(signature = (request, data=None))]
fn aparse<'py>(
    py: Python<'py>,
    request: &Bound<'py, PyDict>,
    data: Option<&Bound<'py, PyBytes>>,
) -> PyResult<Bound<'py, PyAny>> {
    let req = build_request(request, data)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let resp = liteocr_core::parse(req).await.map_err(to_py_err)?;
        Python::attach(|py| to_py(py, &resp))
    })
}

// ---- ocr mode ------------------------------------------------------------------------------------

/// Run an `ocr`-mode request synchronously; returns a `TextResponse` dict (plain text + boxes).
#[pyfunction]
#[pyo3(signature = (request, data=None))]
fn ocr(py: Python<'_>, request: &Bound<'_, PyDict>, data: Option<&Bound<'_, PyBytes>>) -> PyResult<Py<PyAny>> {
    let req = build_request(request, data)?;
    let rt = pyo3_async_runtimes::tokio::get_runtime();
    let resp = py.detach(|| rt.block_on(liteocr_core::ocr(req))).map_err(to_py_err)?;
    to_py(py, &resp)
}

/// Async variant of [`ocr`] returning an awaitable.
#[pyfunction]
#[pyo3(signature = (request, data=None))]
fn aocr<'py>(
    py: Python<'py>,
    request: &Bound<'py, PyDict>,
    data: Option<&Bound<'py, PyBytes>>,
) -> PyResult<Bound<'py, PyAny>> {
    let req = build_request(request, data)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let resp = liteocr_core::ocr(req).await.map_err(to_py_err)?;
        Python::attach(|py| to_py(py, &resp))
    })
}

// ---- extract mode --------------------------------------------------------------------------------

/// Run an `extract`-mode request synchronously; returns an `ExtractResponse` dict.
#[pyfunction]
#[pyo3(signature = (request, data=None))]
fn extract(py: Python<'_>, request: &Bound<'_, PyDict>, data: Option<&Bound<'_, PyBytes>>) -> PyResult<Py<PyAny>> {
    let req = build_extract_request(request, data)?;
    let rt = pyo3_async_runtimes::tokio::get_runtime();
    let resp = py.detach(|| rt.block_on(liteocr_core::extract(req))).map_err(to_py_err)?;
    to_py(py, &resp)
}

/// Async variant of [`extract`] returning an awaitable.
#[pyfunction]
#[pyo3(signature = (request, data=None))]
fn aextract<'py>(
    py: Python<'py>,
    request: &Bound<'py, PyDict>,
    data: Option<&Bound<'py, PyBytes>>,
) -> PyResult<Bound<'py, PyAny>> {
    let req = build_extract_request(request, data)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let resp = liteocr_core::extract(req).await.map_err(to_py_err)?;
        Python::attach(|py| to_py(py, &resp))
    })
}

// ---- asynchronous jobs ---------------------------------------------------------------------------

fn build_job(job: &Bound<'_, PyDict>) -> PyResult<JobHandle> {
    depythonize(job.as_any()).map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("invalid job: {e}")))
}

fn build_retrieve_options(options: Option<&Bound<'_, PyDict>>) -> PyResult<RetrieveOptions> {
    match options {
        None => Ok(RetrieveOptions::default()),
        Some(o) => depythonize(o.as_any())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("invalid retrieve options: {e}"))),
    }
}

/// `Pending` → `{"status": "pending"}`, `Succeeded` → `{"status": "succeeded", "result": {...}}`;
/// a failed job raises `CoreError` like any other failure.
fn job_status_to_py(py: Python<'_>, status: JobStatus) -> PyResult<Py<PyAny>> {
    match status {
        JobStatus::Failed(e) => Err(to_py_err(e)),
        other => to_py(py, &other),
    }
}

/// Start a `parse` job and return the job handle as a dict (see `liteocr_core::submit_parse`).
#[pyfunction]
#[pyo3(signature = (request, data=None))]
fn submit(py: Python<'_>, request: &Bound<'_, PyDict>, data: Option<&Bound<'_, PyBytes>>) -> PyResult<Py<PyAny>> {
    let req = build_request(request, data)?;
    let rt = pyo3_async_runtimes::tokio::get_runtime();
    let job = py.detach(|| rt.block_on(liteocr_core::submit_parse(req))).map_err(to_py_err)?;
    to_py(py, &job)
}

/// Async variant of [`submit`].
#[pyfunction]
#[pyo3(signature = (request, data=None))]
fn asubmit<'py>(
    py: Python<'py>,
    request: &Bound<'py, PyDict>,
    data: Option<&Bound<'py, PyBytes>>,
) -> PyResult<Bound<'py, PyAny>> {
    let req = build_request(request, data)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let job = liteocr_core::submit_parse(req).await.map_err(to_py_err)?;
        Python::attach(|py| to_py(py, &job))
    })
}

/// Check a submitted job once. Returns `{"status": "pending"}` or
/// `{"status": "succeeded", "result": <ParseResponse dict>}`; raises on a failed job.
#[pyfunction]
#[pyo3(signature = (job, options=None))]
fn retrieve(py: Python<'_>, job: &Bound<'_, PyDict>, options: Option<&Bound<'_, PyDict>>) -> PyResult<Py<PyAny>> {
    let job = build_job(job)?;
    let opts = build_retrieve_options(options)?;
    let rt = pyo3_async_runtimes::tokio::get_runtime();
    let status = py.detach(|| rt.block_on(liteocr_core::retrieve_parse_with(&job, &opts))).map_err(to_py_err)?;
    job_status_to_py(py, status)
}

/// Async variant of [`retrieve`].
#[pyfunction]
#[pyo3(signature = (job, options=None))]
fn aretrieve<'py>(
    py: Python<'py>,
    job: &Bound<'py, PyDict>,
    options: Option<&Bound<'py, PyDict>>,
) -> PyResult<Bound<'py, PyAny>> {
    let job = build_job(job)?;
    let opts = build_retrieve_options(options)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let status = liteocr_core::retrieve_parse_with(&job, &opts).await.map_err(to_py_err)?;
        Python::attach(|py| job_status_to_py(py, status))
    })
}

/// Interpret a provider webhook body (no network). Returns `{"job": <job dict> | None,
/// "status": {"status": "pending" | "finished" | "succeeded" | "failed", "result": ...}}`.
#[pyfunction]
fn parse_webhook(py: Python<'_>, model: &str, payload: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    let payload: serde_json::Value = depythonize(payload)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("invalid webhook payload: {e}")))?;
    let event = liteocr_core::parse_webhook(model, &payload).map_err(to_py_err)?;
    to_py(py, &event)
}

// ---- router --------------------------------------------------------------------------------------

/// Multi-model router with fallbacks. All models must support the router's mode.
#[pyclass(name = "Router", module = "liteocr._core")]
struct PyRouter {
    inner: Arc<CoreRouter>,
}

#[pymethods]
impl PyRouter {
    #[new]
    #[pyo3(signature = (models, mode="parse", strategy="ordered", fallback_on=None))]
    fn new(models: Vec<String>, mode: &str, strategy: &str, fallback_on: Option<Vec<String>>) -> PyResult<Self> {
        let strategy: Strategy = strategy.parse().map_err(to_py_err)?;
        let mut config = RouterConfig::new(models).mode(parse_mode(mode)?);
        config.strategy = strategy;
        if let Some(kinds) = fallback_on {
            config.fallback_on = kinds
                .iter()
                .map(|k| serde_json::from_value(serde_json::Value::String(k.clone())))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("invalid fallback_on: {e}")))?;
        }
        Ok(Self { inner: Arc::new(CoreRouter::new(config).map_err(to_py_err)?) })
    }

    fn models(&self) -> Vec<String> {
        self.inner.models()
    }

    fn mode(&self) -> &'static str {
        self.inner.mode().as_str()
    }

    fn plan(&self) -> Vec<String> {
        self.inner.plan()
    }

    #[pyo3(signature = (request, data=None))]
    fn parse(
        &self,
        py: Python<'_>,
        request: &Bound<'_, PyDict>,
        data: Option<&Bound<'_, PyBytes>>,
    ) -> PyResult<Py<PyAny>> {
        let req = build_request(request, data)?;
        let router = self.inner.clone();
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        let resp = py.detach(|| rt.block_on(router.parse(&req))).map_err(to_py_err)?;
        to_py(py, &resp)
    }

    #[pyo3(signature = (request, data=None))]
    fn aparse<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'py, PyDict>,
        data: Option<&Bound<'py, PyBytes>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let req = build_request(request, data)?;
        let router = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let resp = router.parse(&req).await.map_err(to_py_err)?;
            Python::attach(|py| to_py(py, &resp))
        })
    }

    #[pyo3(signature = (request, data=None))]
    fn ocr(
        &self,
        py: Python<'_>,
        request: &Bound<'_, PyDict>,
        data: Option<&Bound<'_, PyBytes>>,
    ) -> PyResult<Py<PyAny>> {
        let req = build_request(request, data)?;
        let router = self.inner.clone();
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        let resp = py.detach(|| rt.block_on(router.ocr(&req))).map_err(to_py_err)?;
        to_py(py, &resp)
    }

    #[pyo3(signature = (request, data=None))]
    fn aocr<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'py, PyDict>,
        data: Option<&Bound<'py, PyBytes>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let req = build_request(request, data)?;
        let router = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let resp = router.ocr(&req).await.map_err(to_py_err)?;
            Python::attach(|py| to_py(py, &resp))
        })
    }

    #[pyo3(signature = (request, data=None))]
    fn extract(
        &self,
        py: Python<'_>,
        request: &Bound<'_, PyDict>,
        data: Option<&Bound<'_, PyBytes>>,
    ) -> PyResult<Py<PyAny>> {
        let req = build_extract_request(request, data)?;
        let router = self.inner.clone();
        let rt = pyo3_async_runtimes::tokio::get_runtime();
        let resp = py.detach(|| rt.block_on(router.extract(&req))).map_err(to_py_err)?;
        to_py(py, &resp)
    }

    #[pyo3(signature = (request, data=None))]
    fn aextract<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'py, PyDict>,
        data: Option<&Bound<'py, PyBytes>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let req = build_extract_request(request, data)?;
        let router = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let resp = router.extract(&req).await.map_err(to_py_err)?;
            Python::attach(|py| to_py(py, &resp))
        })
    }

    fn stats(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        to_py(py, &self.inner.stats())
    }

    fn __repr__(&self) -> String {
        format!("Router(models={:?}, mode={:?})", self.inner.models(), self.inner.mode().as_str())
    }
}

// ---- native-format compatibility -----------------------------------------------------------------

/// Every value `output_format` accepts, in documentation order.
///
/// The SDK and CLI quote this list in their error messages, so it has exactly one source.
#[pyfunction]
fn output_formats() -> Vec<&'static str> {
    Format::ALL.iter().map(Format::as_str).collect()
}

/// Fail fast on an unknown `output_format`, before any provider call.
#[pyfunction]
fn validate_output_format(format: &str) -> PyResult<String> {
    format.parse::<Format>().map(|f| f.as_str().to_string()).map_err(to_py_err)
}

/// Render an already-computed unified `ParseResponse` dict in a provider's native JSON shape.
///
/// The rendering itself lives in `liteocr_core::compat`; this only bridges dict <-> struct so the
/// Python package never has to know a vendor's payload layout.
#[pyfunction]
fn render_parse(py: Python<'_>, response: &Bound<'_, PyDict>, format: &str) -> PyResult<Py<PyAny>> {
    let resp: ParseResponse = depythonize(response.as_any())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("invalid parse response: {e}")))?;
    let value = resp.to_format(format).map_err(to_py_err)?;
    to_py(py, &value)
}

/// Render an already-computed unified `ExtractResponse` dict in a provider's native JSON shape.
#[pyfunction]
fn render_extract(py: Python<'_>, response: &Bound<'_, PyDict>, format: &str) -> PyResult<Py<PyAny>> {
    let resp: ExtractResponse = depythonize(response.as_any())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("invalid extract response: {e}")))?;
    let value = resp.to_format(format).map_err(to_py_err)?;
    to_py(py, &value)
}

// ---- registry, pricing, helpers ------------------------------------------------------------------

/// All fully-qualified model names, optionally restricted to the ones serving `mode`.
#[pyfunction]
#[pyo3(signature = (mode=None))]
fn list_models(mode: Option<&str>) -> PyResult<Vec<String>> {
    match mode {
        Some(m) => Ok(liteocr_core::list_models_for(parse_mode(m)?)),
        None => Ok(liteocr_core::list_models()),
    }
}

/// The known modes, in order.
#[pyfunction]
fn modes() -> Vec<&'static str> {
    Mode::ALL.iter().map(Mode::as_str).collect()
}

#[pyfunction]
fn providers(py: Python<'_>) -> PyResult<Py<PyAny>> {
    to_py(py, &liteocr_core::PROVIDERS)
}

#[pyfunction]
fn pricing(py: Python<'_>) -> PyResult<Py<PyAny>> {
    to_py(py, &liteocr_core::pricing::all_prices())
}

#[pyfunction]
#[pyo3(signature = (prices, mode="parse"))]
fn set_pricing(prices: std::collections::BTreeMap<String, f64>, mode: &str) -> PyResult<()> {
    liteocr_core::pricing::set_prices(prices, parse_mode(mode)?);
    Ok(())
}

#[pyfunction]
fn reset_pricing() {
    liteocr_core::pricing::reset_prices();
}

#[pyfunction]
#[pyo3(signature = (model, mode, pages))]
fn estimate_cost(model: &str, mode: &str, pages: u32) -> PyResult<Option<f64>> {
    Ok(liteocr_core::pricing::estimate_cost(model, parse_mode(mode)?, pages))
}

/// Validate and canonicalise a model string ("reducto" -> "reducto/standard"). With `mode`, the
/// model must support it and a bare provider resolves to its default model for that mode.
#[pyfunction]
#[pyo3(signature = (model, mode=None))]
fn resolve_model(model: &str, mode: Option<&str>) -> PyResult<String> {
    let parsed = match mode {
        Some(m) => liteocr_core::ModelRef::parse_for(model, parse_mode(m)?),
        None => liteocr_core::ModelRef::parse(model),
    };
    parsed.map(|m| m.qualified()).map_err(to_py_err)
}

/// Benchmark metrics between a prediction and ground truth.
#[pyfunction]
#[pyo3(signature = (prediction, truth, case_insensitive=true, strip_markdown=true, strip_punctuation=false))]
fn score(
    py: Python<'_>,
    prediction: &str,
    truth: &str,
    case_insensitive: bool,
    strip_markdown: bool,
    strip_punctuation: bool,
) -> PyResult<Py<PyAny>> {
    let opts = liteocr_core::bench::NormalizeOptions { case_insensitive, strip_markdown, strip_punctuation };
    let m = py.detach(|| liteocr_core::bench::score(prediction, truth, opts));
    to_py(py, &m)
}

#[pyfunction]
#[pyo3(signature = (text, case_insensitive=true, strip_markdown=true, strip_punctuation=false))]
fn normalize_text(text: &str, case_insensitive: bool, strip_markdown: bool, strip_punctuation: bool) -> String {
    let opts = liteocr_core::bench::NormalizeOptions { case_insensitive, strip_markdown, strip_punctuation };
    liteocr_core::bench::normalize(text, opts)
}

#[pyfunction]
fn markdown_to_text(md: &str) -> String {
    liteocr_core::types::markdown_to_text(md)
}

/// Enable core tracing output to stderr at the given level (e.g. "info", "debug").
#[pyfunction]
fn init_logging(level: &str) -> PyResult<()> {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_new(level).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    Ok(())
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", liteocr_core::VERSION)?;
    m.add("CoreError", m.py().get_type::<CoreError>())?;
    m.add_class::<PyRouter>()?;
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(aparse, m)?)?;
    m.add_function(wrap_pyfunction!(ocr, m)?)?;
    m.add_function(wrap_pyfunction!(aocr, m)?)?;
    m.add_function(wrap_pyfunction!(extract, m)?)?;
    m.add_function(wrap_pyfunction!(aextract, m)?)?;
    m.add_function(wrap_pyfunction!(submit, m)?)?;
    m.add_function(wrap_pyfunction!(asubmit, m)?)?;
    m.add_function(wrap_pyfunction!(retrieve, m)?)?;
    m.add_function(wrap_pyfunction!(aretrieve, m)?)?;
    m.add_function(wrap_pyfunction!(parse_webhook, m)?)?;
    m.add_function(wrap_pyfunction!(output_formats, m)?)?;
    m.add_function(wrap_pyfunction!(validate_output_format, m)?)?;
    m.add_function(wrap_pyfunction!(render_parse, m)?)?;
    m.add_function(wrap_pyfunction!(render_extract, m)?)?;
    m.add_function(wrap_pyfunction!(list_models, m)?)?;
    m.add_function(wrap_pyfunction!(modes, m)?)?;
    m.add_function(wrap_pyfunction!(providers, m)?)?;
    m.add_function(wrap_pyfunction!(pricing, m)?)?;
    m.add_function(wrap_pyfunction!(set_pricing, m)?)?;
    m.add_function(wrap_pyfunction!(reset_pricing, m)?)?;
    m.add_function(wrap_pyfunction!(estimate_cost, m)?)?;
    m.add_function(wrap_pyfunction!(resolve_model, m)?)?;
    m.add_function(wrap_pyfunction!(score, m)?)?;
    m.add_function(wrap_pyfunction!(normalize_text, m)?)?;
    m.add_function(wrap_pyfunction!(markdown_to_text, m)?)?;
    m.add_function(wrap_pyfunction!(init_logging, m)?)?;
    Ok(())
}
