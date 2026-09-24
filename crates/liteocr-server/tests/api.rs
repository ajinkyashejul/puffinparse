//! Gateway handler tests. The app is driven with `tower::ServiceExt::oneshot`; the provider is a
//! loopback mock of Mistral's `POST /v1/ocr` (one synchronous call, real redacted fixture payload)
//! reached through the gateway's per-provider / per-target `base_url` override.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::routing::post;
use http_body_util::BodyExt;
use liteocr_server::{AppState, Config};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const FIXTURE: &str = include_str!("../../liteocr-core/tests/fixtures/mistral_ocr.json");
/// "iVBORw0KGgo=" is the PNG signature; the mock never looks at the bytes.
const PNG_B64: &str = "iVBORw0KGgo=";

#[derive(Default)]
struct Mock {
    ok_hits: AtomicUsize,
    fail_hits: AtomicUsize,
    auth_seen: Mutex<Vec<String>>,
}

/// Start the mock provider; returns its base address (`http://127.0.0.1:port`).
async fn mock() -> (String, Arc<Mock>) {
    let m = Arc::new(Mock::default());
    let (a, b) = (m.clone(), m.clone());
    let app = axum::Router::new()
        .route(
            "/ok/v1/ocr",
            post(move |headers: axum::http::HeaderMap| async move {
                a.ok_hits.fetch_add(1, Ordering::SeqCst);
                let auth = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                a.auth_seen.lock().unwrap().push(auth);
                ([(header::CONTENT_TYPE, "application/json")], FIXTURE)
            }),
        )
        .route(
            "/fail/v1/ocr",
            post(move || async move {
                b.fail_hits.fetch_add(1, Ordering::SeqCst);
                (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!({ "message": "mock upstream exploded" })))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), m)
}

fn config(base: &str) -> String {
    std::env::set_var("LITEOCR_TEST_MISTRAL_KEY", "provider-key-from-env");
    format!(
        r#"
        master_key = "sk-master"
        [server]
        max_retries = 0
        [providers.mistral]
        api_key = "env:LITEOCR_TEST_MISTRAL_KEY"
        base_url = "{base}/ok"

        [[models]]
        name = "resilient"
        targets = [{{ model = "mistral/ocr-latest", base_url = "{base}/fail" }}, "mistral/ocr-2512"]

        [[models]]
        name = "brittle"
        targets = [{{ model = "mistral/ocr-latest", base_url = "{base}/fail" }}, "mistral/ocr-2512"]
        fallback_on = ["timeout"]

        [[keys]]
        id = "team-a"
        key = "sk-team-a"
        models = ["resilient", "mistral/*"]

        [[keys]]
        id = "broke"
        key = "sk-broke"
        monthly_budget_usd = 0.005

        [[keys]]
        id = "slow"
        key = "sk-slow"
        rpm = 1

        [[keys]]
        id = "narrow"
        key = "sk-narrow"
        models = ["brittle"]
        "#
    )
}

async fn app() -> (axum::Router, Arc<Mock>) {
    let (base, m) = mock().await;
    let state = AppState::from_config_quiet(Config::from_toml(&config(&base)).unwrap()).unwrap();
    (liteocr_server::app(Arc::new(state)), m)
}

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Value, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, headers, json, text)
}

fn post_json(path: &str, key: Option<&str>, body: Value) -> Request<Body> {
    let mut b = Request::post(path).header(header::CONTENT_TYPE, "application/json");
    if let Some(k) = key {
        b = b.header(header::AUTHORIZATION, format!("Bearer {k}"));
    }
    b.body(Body::from(body.to_string())).unwrap()
}

fn doc(model: &str) -> Value {
    json!({ "model": model, "document": PNG_B64, "filename": "page.png" })
}

#[tokio::test]
async fn health_is_open_and_parse_requires_a_key() {
    let (app, m) = app().await;
    let (s, _, body, _) = send(&app, Request::get("/health").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "ok");

    let (s, h, body, _) = send(&app, post_json("/v1/parse", None, doc("mistral"))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["type"], "unauthorized");
    assert!(body["error"]["request_id"].is_string());
    assert!(h.contains_key("x-request-id"));

    let (s, _, _, _) = send(&app, post_json("/v1/parse", Some("sk-wrong"), doc("mistral"))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(m.ok_hits.load(Ordering::SeqCst), 0, "no provider call before auth");
}

#[tokio::test]
async fn parse_returns_unified_json_with_cost() {
    let (app, m) = app().await;
    let (s, h, body, text) = send(&app, post_json("/v1/parse", Some("sk-team-a"), doc("mistral/ocr-latest"))).await;
    assert_eq!(s, StatusCode::OK, "{text}");
    assert_eq!(body["model"], "mistral/ocr-latest");
    assert_eq!(body["provider"], "mistral");
    assert_eq!(body["usage"]["pages"], 2);
    assert!(body["markdown"].as_str().unwrap().contains("Hello LiteOCR"));
    assert!((body["cost_usd"].as_f64().unwrap() - 0.008).abs() < 1e-9);
    assert_eq!(h["x-liteocr-model"], "mistral/ocr-latest");
    // The provider key came from the `env:` reference in [providers.mistral].
    assert_eq!(m.auth_seen.lock().unwrap().as_slice(), ["Bearer provider-key-from-env"]);

    // `x-api-key` works too, and the usage endpoint reflects the spend.
    let req = Request::get("/v1/usage").header("x-api-key", "sk-team-a").body(Body::empty()).unwrap();
    let (s, _, usage, _) = send(&app, req).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(usage["keys"].as_array().unwrap().len(), 1);
    assert_eq!(usage["keys"][0]["requests"], 1);
    assert_eq!(usage["keys"][0]["pages"], 2);
}

#[tokio::test]
async fn ocr_mode_and_multipart_upload() {
    let (app, _) = app().await;
    let boundary = "XBOUNDARYX";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nmistral\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"pages\"\r\n\r\n1-2\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"scan.png\"\r\n\
         Content-Type: image/png\r\n\r\n\u{89}PNG\r\n--{boundary}--\r\n"
    );
    let req = Request::post("/v1/ocr")
        .header(header::AUTHORIZATION, "Bearer sk-master")
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    let (s, _, body, text) = send(&app, req).await;
    assert_eq!(s, StatusCode::OK, "{text}");
    assert!(body["text"].as_str().unwrap().contains("Invoice #1234"));
    assert!(body["pages"][0]["lines"].is_array());
}

#[tokio::test]
async fn output_format_renders_the_vendor_shape() {
    let (app, _) = app().await;
    let mut req = doc("mistral");
    req["output_format"] = json!("reducto");
    let (s, _, body, text) = send(&app, post_json("/v1/parse", Some("sk-master"), req)).await;
    assert_eq!(s, StatusCode::OK, "{text}");
    assert_eq!(body["response_type"], "parse");
    assert_eq!(body["usage"]["num_pages"], 2);
    assert!(body["result"]["chunks"].is_array());

    let mut bad = doc("mistral");
    bad["output_format"] = json!("acme");
    let (s, _, body, _) = send(&app, post_json("/v1/parse", Some("sk-master"), bad)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "input_error");

    let mut ocr = doc("mistral");
    ocr["output_format"] = json!("reducto");
    let (s, _, _, _) = send(&app, post_json("/v1/ocr", Some("sk-master"), ocr)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn alias_falls_back_on_provider_error() {
    let (app, m) = app().await;
    let (s, _, body, text) = send(&app, post_json("/v1/parse", Some("sk-team-a"), doc("resilient"))).await;
    assert_eq!(s, StatusCode::OK, "{text}");
    assert_eq!(body["model"], "mistral/ocr-2512");
    assert_eq!(body["metadata"]["liteocr_fallback_index"], 1);
    assert!(body["metadata"]["liteocr_fallback_from_error"].as_str().unwrap().contains("mock upstream exploded"));
    assert_eq!(m.fail_hits.load(Ordering::SeqCst), 1);
    assert_eq!(m.ok_hits.load(Ordering::SeqCst), 1);

    let metrics = send(&app, Request::get("/metrics").body(Body::empty()).unwrap()).await.3;
    assert!(metrics.contains("liteocr_fallbacks_total 1"), "{metrics}");
}

#[tokio::test]
async fn provider_error_without_fallback_keeps_the_provider_message() {
    let (app, m) = app().await;
    let (s, _, body, _) = send(&app, post_json("/v1/parse", Some("sk-narrow"), doc("brittle"))).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"]["type"], "provider_error");
    assert_eq!(body["error"]["message"], "mock upstream exploded");
    assert_eq!(body["error"]["provider"], "mistral");
    assert_eq!(body["error"]["provider_status"], 500);
    assert_eq!(m.ok_hits.load(Ordering::SeqCst), 0, "fallback_on = [timeout] must not fall back on a 500");
}

#[tokio::test]
async fn request_fallbacks_extend_the_plan() {
    let (app, m) = app().await;
    let mut req = doc("brittle");
    req["fallbacks"] = json!(["mistral/ocr-4-1"]);
    // `brittle`'s own fallback_on (timeout only) governs the whole plan, so no fallback here...
    let (s, _, _, _) = send(&app, post_json("/v1/parse", Some("sk-master"), req)).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    // ...while an alias with the default kinds falls back within its own targets first.
    let (base_fail_hits, base_ok_hits) = (m.fail_hits.load(Ordering::SeqCst), m.ok_hits.load(Ordering::SeqCst));
    let mut req = doc("resilient");
    req["fallbacks"] = json!(["mistral/ocr-4-1"]);
    let (s, _, body, _) = send(&app, post_json("/v1/parse", Some("sk-master"), req)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["model"], "mistral/ocr-2512");
    assert_eq!(m.fail_hits.load(Ordering::SeqCst), base_fail_hits + 1);
    assert_eq!(m.ok_hits.load(Ordering::SeqCst), base_ok_hits + 1);
}

#[tokio::test]
async fn key_model_allow_list_is_enforced() {
    let (app, m) = app().await;
    let (s, _, body, _) = send(&app, post_json("/v1/parse", Some("sk-narrow"), doc("mistral/ocr-latest"))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["type"], "model_not_allowed");
    // Fallbacks are checked too.
    let mut req = doc("brittle");
    req["fallbacks"] = json!(["mistral/ocr-latest"]);
    let (s, _, _, _) = send(&app, post_json("/v1/parse", Some("sk-narrow"), req)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(m.ok_hits.load(Ordering::SeqCst) + m.fail_hits.load(Ordering::SeqCst), 0);

    let req = Request::get("/v1/models").header(header::AUTHORIZATION, "Bearer sk-narrow").body(Body::empty()).unwrap();
    let (s, _, models, _) = send(&app, req).await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&str> = models["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["brittle"]);

    let req = Request::get("/v1/models").header(header::AUTHORIZATION, "Bearer sk-master").body(Body::empty()).unwrap();
    let (_, _, models, _) = send(&app, req).await;
    let reducto = models["data"].as_array().unwrap().iter().find(|m| m["id"] == "reducto/standard").unwrap();
    assert!(reducto["per_page_usd"]["parse"].is_number());
    assert_eq!(reducto["modes"], json!(["parse", "ocr"]));
}

#[tokio::test]
async fn budget_exhaustion_returns_402() {
    let (app, m) = app().await;
    // Budget $0.005; one 2-page call at $0.004/page spends $0.008.
    let (s, _, _, _) = send(&app, post_json("/v1/parse", Some("sk-broke"), doc("mistral"))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, body, _) = send(&app, post_json("/v1/parse", Some("sk-broke"), doc("mistral"))).await;
    assert_eq!(s, StatusCode::PAYMENT_REQUIRED);
    assert_eq!(body["error"]["type"], "budget_exceeded");
    assert_eq!(m.ok_hits.load(Ordering::SeqCst), 1);
    // The master key has no budget.
    let (s, _, _, _) = send(&app, post_json("/v1/parse", Some("sk-master"), doc("mistral"))).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn rate_limit_returns_429_with_retry_after() {
    let (app, _) = app().await;
    let (s, _, _, _) = send(&app, post_json("/v1/parse", Some("sk-slow"), doc("mistral"))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, h, body, _) = send(&app, post_json("/v1/parse", Some("sk-slow"), doc("mistral"))).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["type"], "key_rate_limited");
    let retry: u64 = h[header::RETRY_AFTER].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry));
}

#[tokio::test]
async fn rejects_bad_requests_before_calling_a_provider() {
    let (app, m) = app().await;
    let cases = [
        // clients may not redirect the gateway's provider credentials
        json!({ "model": "mistral", "document_url": "https://x/a.pdf", "base_url": "http://evil" }),
        json!({ "model": "mistral", "document_url": "https://x/a.pdf", "api_key": "k" }),
        json!({ "document_url": "https://x/a.pdf" }),
        json!({ "model": "mistral" }),
        json!({ "model": "mistral", "document": PNG_B64 }),
        json!({ "model": "mistral", "document_url": "file:///etc/passwd" }),
        json!({ "model": "mistral", "document": "@@@", "filename": "a.png" }),
        json!({ "model": "nope/x", "document_url": "https://x/a.pdf" }),
        json!({ "model": "mistral", "document_url": "https://x/a.pdf", "schema": {} }),
    ];
    for c in cases {
        let (s, _, body, _) = send(&app, post_json("/v1/parse", Some("sk-master"), c.clone())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{c}: {body}");
        assert!(body["error"]["message"].is_string());
    }
    // extract needs a schema; a parse-only alias cannot serve extract... datalab has no extract.
    let (s, _, _, _) = send(
        &app,
        post_json("/v1/extract", Some("sk-master"), json!({ "model": "mistral", "document_url": "https://x/a.pdf" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, body, _) = send(
        &app,
        post_json(
            "/v1/extract",
            Some("sk-master"),
            json!({ "model": "datalab", "document_url": "https://x/a.pdf", "schema": {"type": "object"} }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "unsupported_model_error");
    assert_eq!(m.ok_hits.load(Ordering::SeqCst) + m.fail_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn metrics_count_requests_errors_pages_and_cost() {
    let (app, _) = app().await;
    send(&app, post_json("/v1/parse", Some("sk-master"), doc("mistral"))).await;
    send(&app, post_json("/v1/parse", None, doc("mistral"))).await;
    let (s, h, _, text) = send(&app, Request::get("/metrics").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/plain"));
    assert!(
        text.contains(r#"liteocr_requests_total{mode="parse",model="mistral/ocr-latest",status="200"} 1"#),
        "{text}"
    );
    assert!(text.contains(r#"liteocr_requests_total{mode="parse",model="-",status="401"} 1"#), "{text}");
    assert!(text.contains(r#"liteocr_errors_total{type="unauthorized"} 1"#));
    assert!(text.contains(r#"liteocr_pages_total{model="mistral/ocr-latest"} 2"#));
    assert!(text.contains(r#"liteocr_cost_usd_total{model="mistral/ocr-latest"} 0.008"#));
    assert!(text.contains(r#"liteocr_request_duration_seconds_count{mode="parse"} 2"#));
}

#[tokio::test]
async fn open_gateway_without_keys() {
    let (base, _) = mock().await;
    let cfg = Config::from_toml(&format!("[providers.mistral]\napi_key = \"k\"\nbase_url = \"{base}/ok\"")).unwrap();
    let app = liteocr_server::app(Arc::new(AppState::from_config_quiet(cfg).unwrap()));
    let (s, _, _, _) = send(&app, post_json("/v1/parse", None, doc("mistral"))).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn aliases_only_mode_rejects_direct_models() {
    let (base, _) = mock().await;
    let cfg = Config::from_toml(&format!(
        "[server]\nallow_direct_models = false\n[providers.mistral]\napi_key = \"k\"\nbase_url = \"{base}/ok\"\n\
         [[models]]\nname = \"ocr\"\ntargets = [\"mistral/ocr-latest\"]"
    ))
    .unwrap();
    let app = liteocr_server::app(Arc::new(AppState::from_config_quiet(cfg).unwrap()));
    let (s, _, body, _) = send(&app, post_json("/v1/parse", None, doc("mistral/ocr-latest"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "unsupported_model_error");
    let (s, _, _, _) = send(&app, post_json("/v1/parse", None, doc("ocr"))).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn request_log_has_ids_but_no_secrets_or_content() {
    let (base, _) = mock().await;
    let dir = std::env::temp_dir().join(format!("liteocr-log-{}", uuid_like()));
    std::fs::create_dir_all(&dir).unwrap();
    let log_path = dir.join("requests.jsonl");
    let mut cfg = Config::from_toml(&config(&base)).unwrap();
    cfg.server.log_stdout = false;
    cfg.server.log_file = Some(log_path.clone());
    let app = liteocr_server::app(Arc::new(AppState::from_config(cfg).unwrap()));
    let req = Request::post("/v1/parse")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, "Bearer sk-team-a")
        .header("x-request-id", "req-123")
        .body(Body::from(doc("mistral").to_string()))
        .unwrap();
    let (s, h, _, _) = send(&app, req).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h["x-request-id"], "req-123");
    send(&app, post_json("/v1/parse", Some("sk-narrow"), doc("brittle"))).await;

    let text = std::fs::read_to_string(&log_path).unwrap();
    let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["request_id"], "req-123");
    assert_eq!(lines[0]["key_id"], "team-a");
    assert_eq!(lines[0]["served_model"], "mistral/ocr-latest");
    assert_eq!(lines[0]["provider"], "mistral");
    assert_eq!(lines[0]["pages"], 2);
    assert_eq!(lines[0]["status"], 200);
    assert!(lines[0]["latency_ms"].is_u64());
    assert_eq!(lines[1]["key_id"], "narrow");
    assert_eq!(lines[1]["status"], 502);
    assert_eq!(lines[1]["error_type"], "provider_error");
    assert_eq!(lines[1]["provider_status"], 500);
    for secret in ["sk-team-a", "sk-narrow", "sk-master", "provider-key-from-env", PNG_B64, "Hello LiteOCR", "exploded"]
    {
        assert!(!text.contains(secret), "log leaked {secret}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

fn uuid_like() -> String {
    format!("{}-{:?}", std::process::id(), std::time::SystemTime::now())
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

#[tokio::test]
async fn oversized_body_is_413() {
    let cfg = Config::from_toml("[server]\nmax_body_mb = 1").unwrap();
    let app = liteocr_server::app(Arc::new(AppState::from_config_quiet(cfg).unwrap()));
    let big = "A".repeat(2 * 1024 * 1024);
    let req = post_json("/v1/parse", None, json!({ "model": "mistral", "document": big, "filename": "a.png" }));
    let (s, _, body, _) = send(&app, req).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"]["type"], "payload_too_large");
}
