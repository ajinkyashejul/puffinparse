//! PyO3 bindings: the `liteocr._core` extension module.
//!
//! The Python package (`python/liteocr`) wraps these functions with a typed, idiomatic API.
//! All heavy lifting stays in `liteocr-core`; this layer only converts values and bridges runtimes.

use liteocr_core::router::{Router as CoreRouter, RouterConfig, Strategy};
use liteocr_core::{DocumentInput, OcrRequest};
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use pythonize::{depythonize, pythonize};
use std::sync::Arc;

pyo3::create_exception!(_core, CoreError, PyException, "Raised by the core; args[0] is a JSON-encoded error.");

fn to_py_err(e: liteocr_core::Error) -> PyErr {
    CoreError::new_err(serde_json::to_string(&e).unwrap_or_else(|_| e.to_string()))
}

fn build_request(request: &Bound<'_, PyDict>, data: Option<&Bound<'_, PyBytes>>) -> PyResult<OcrRequest> {
    let mut req: OcrRequest = depythonize(request.as_any())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("invalid request: {e}")))?;
    if let Some(bytes) = data {
        let filename = match &req.input {
            DocumentInput::Bytes { filename, .. } => filename.clone(),
            other => other.filename(),
        };
        req.input = DocumentInput::Bytes { data: bytes::Bytes::copy_from_slice(bytes.as_bytes()), filename };
    }
    Ok(req)
}

fn response_to_py(py: Python<'_>, resp: liteocr_core::OcrResponse) -> PyResult<Py<PyAny>> {
    Ok(pythonize(py, &resp)?.unbind())
}

/// Run an OCR request synchronously. `request` is a dict matching `OcrRequest`; `data` optionally
/// supplies the document bytes (avoids base64-encoding large files through the dict).
#[pyfunction]
#[pyo3(signature = (request, data=None))]
fn ocr(py: Python<'_>, request: &Bound<'_, PyDict>, data: Option<&Bound<'_, PyBytes>>) -> PyResult<Py<PyAny>> {
    let req = build_request(request, data)?;
    let rt = pyo3_async_runtimes::tokio::get_runtime();
    let result = py.detach(|| rt.block_on(liteocr_core::ocr(req)));
    let resp = result.map_err(to_py_err)?;
    response_to_py(py, resp)
}

/// Async variant returning an awaitable.
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
        Python::attach(|py| response_to_py(py, resp))
    })
}

/// Multi-model router with fallbacks.
#[pyclass(name = "Router", module = "liteocr._core")]
struct PyRouter {
    inner: Arc<CoreRouter>,
}

#[pymethods]
impl PyRouter {
    #[new]
    #[pyo3(signature = (models, strategy="ordered", fallback_on=None))]
    fn new(models: Vec<String>, strategy: &str, fallback_on: Option<Vec<String>>) -> PyResult<Self> {
        let strategy: Strategy = strategy.parse().map_err(to_py_err)?;
        let mut config = RouterConfig::new(models);
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

    fn plan(&self) -> Vec<String> {
        self.inner.plan()
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
        let result = py.detach(|| rt.block_on(router.ocr(&req)));
        response_to_py(py, result.map_err(to_py_err)?)
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
            Python::attach(|py| response_to_py(py, resp))
        })
    }

    fn stats(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        Ok(pythonize(py, &self.inner.stats())?.unbind())
    }

    fn __repr__(&self) -> String {
        format!("Router(models={:?})", self.inner.models())
    }
}

#[pyfunction]
fn list_models() -> Vec<String> {
    liteocr_core::list_models()
}

#[pyfunction]
fn providers(py: Python<'_>) -> PyResult<Py<PyAny>> {
    Ok(pythonize(py, liteocr_core::PROVIDERS)?.unbind())
}

#[pyfunction]
fn pricing(py: Python<'_>) -> PyResult<Py<PyAny>> {
    Ok(pythonize(py, &liteocr_core::pricing::all_prices())?.unbind())
}

#[pyfunction]
fn set_pricing(prices: std::collections::BTreeMap<String, f64>) {
    liteocr_core::pricing::set_prices(prices);
}

#[pyfunction]
fn reset_pricing() {
    liteocr_core::pricing::reset_prices();
}

#[pyfunction]
fn estimate_cost(model: &str, pages: u32) -> Option<f64> {
    liteocr_core::pricing::estimate_cost(model, pages)
}

/// Validate and canonicalise a model string ("reducto" -> "reducto/standard").
#[pyfunction]
fn resolve_model(model: &str) -> PyResult<String> {
    liteocr_core::ModelRef::parse(model).map(|m| m.qualified()).map_err(to_py_err)
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
    Ok(pythonize(py, &m)?.unbind())
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
    m.add_function(wrap_pyfunction!(ocr, m)?)?;
    m.add_function(wrap_pyfunction!(aocr, m)?)?;
    m.add_function(wrap_pyfunction!(list_models, m)?)?;
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
