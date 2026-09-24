# LiteOCR gateway (`liteocr serve`)

A small HTTP server in front of every provider LiteOCR supports, in the spirit of the LiteLLM
proxy: clients call one endpoint with one API shape and a gateway-issued key; the gateway holds
the provider keys, routes aliases to provider models with fallbacks, enforces per-key model
lists, monthly budgets and rate limits, and emits JSON-lines request logs and Prometheus metrics.

It is a thin layer over `liteocr-core` (crate `crates/liteocr-server`, axum + tower-http). It adds
no provider logic: every call is `liteocr_core::parse / ocr / extract` with the request fields
below, so responses are exactly the unified types of [SPEC §5](SPEC.md#5-unified-responses), or
a vendor shape via `output_format` ([COMPAT.md](COMPAT.md)).

## Run it

```bash
export LITEOCR_MASTER_KEY=sk-master-change-me
export LITEOCR_KEY_BILLING=sk-billing-change-me LITEOCR_KEY_RESEARCH=sk-research-change-me
export REDUCTO_API_KEY=... EXTEND_API_KEY=... LLAMA_API_KEY=...
liteocr serve --config examples/server/liteocr.toml          # --host / --port override the file
```

Without `--config` it reads `./liteocr.toml` if present (or `$LITEOCR_CONFIG`), else runs with
defaults: `127.0.0.1:4000`, no aliases, **no auth** (a warning is printed). Anything reachable
beyond localhost should have a `master_key` or `[[keys]]`.

Docker (multi-stage build, distroless runtime, runs as non-root):

```bash
docker build -t liteocr .
docker run --rm -p 4000:4000 -v $PWD/examples/server/liteocr.toml:/etc/liteocr/liteocr.toml:ro \
  -e LITEOCR_MASTER_KEY -e LITEOCR_KEY_BILLING -e LITEOCR_KEY_RESEARCH \
  -e REDUCTO_API_KEY -e EXTEND_API_KEY -e LLAMA_API_KEY liteocr
```

The image's default command is `serve --host 0.0.0.0 --config /etc/liteocr/liteocr.toml`; it exits
if no config is mounted rather than serving an open gateway.

## Configuration (`liteocr.toml`)

Every secret may be written as `"env:VAR"`, resolved once at startup. A virtual key or master key
whose reference resolves to nothing is a startup error (fail closed); a provider key that resolves
to nothing prints a warning and the core falls back to the provider's default env var.

```toml
master_key = "env:LITEOCR_MASTER_KEY"   # full access: all models, no budget, no rate limit

[server]
host = "127.0.0.1"
port = 4000
log_stdout = true                        # JSON-lines request log on stdout
log_file = "requests.jsonl"              # optional append-only copy
state_file = "usage.json"                # optional: per-key monthly spend survives restarts
max_body_mb = 50                         # multipart upload / base64 JSON body cap
max_timeout_secs = 300                   # cap (and default) for a request's `timeout`
max_retries = 2                          # per provider call, when the client sends none
allow_direct_models = true               # false: only the aliases below may be requested

[providers.reducto]                      # one table per provider (name as in `liteocr providers`)
api_key = "env:REDUCTO_API_KEY"
# base_url = "https://eu.platform.reducto.ai"

[[models]]                               # an alias clients can send as "model"
name = "invoices"
targets = ["reducto/standard", { model = "extend/parse_performance", api_key = "env:EXTEND_KEY_2" }]
strategy = "ordered"                     # or "round_robin"
fallback_on = ["provider", "rate_limit", "timeout", "network"]   # the default

[[keys]]
id = "billing-team"                      # appears in logs, metrics, usage; never the secret
key = "env:LITEOCR_KEY_BILLING"          # the bearer token the client sends
models = ["invoices", "llamaparse/*"]    # aliases, provider/model, provider/*, or "*"; empty = all
monthly_budget_usd = 50.0                # calendar month, UTC
rpm = 60                                 # requests per minute, sliding 60 s window
```

A complete sample is [`examples/server/liteocr.toml`](../examples/server/liteocr.toml).

**Routing.** `model` is looked up as an alias first, then (if `allow_direct_models`) as a registry
model (`reducto/standard`, or a bare provider for its default model in that mode). An alias expands
to its targets: `ordered` always starts at the first, `round_robin` rotates the start per request,
and both fall back in order when the error kind is in `fallback_on`. A request's own `fallbacks`
list is appended after that (aliases expand in order). Every target is checked against the
endpoint's mode before any provider is called. Target-level `api_key` / `base_url` override the
`[providers.*]` ones, so two deployments of the same provider (two accounts, two regions) can sit
behind one alias. When a fallback served the call, the response metadata carries
`liteocr_fallback_index` and `liteocr_fallback_from_error`, as with the SDK `Router`.

**Budgets.** Spend is the response's `cost_usd` (provider-reported cost when available, otherwise
the list-price estimate from `pricing.json`), summed per key per UTC calendar month. The check runs
before the call: a key whose spend has reached its budget gets `402`. One in-flight request can take
a key past its budget; nothing is charged for failed attempts (a provider may still bill a failed
job). State is in memory; with `state_file` it is rewritten (temp file + rename) after every billed
request and reloaded on startup. Rate-limit windows are not persisted.

## API

Authenticate with `Authorization: Bearer <key>` or `x-api-key: <key>`. Send `x-request-id` to
choose the request id (echoed in the `x-request-id` response header and the log), otherwise a
UUID is generated.

### `POST /v1/parse`, `POST /v1/ocr`, `POST /v1/extract`

Body is JSON or `multipart/form-data` with the same field names (multipart fields are text; the
JSON-valued ones — `provider_options`, `schema`, `metadata`, `fallbacks` — are JSON strings, and
`fallbacks` may also be comma-separated).

| Field | Type | Notes |
|---|---|---|
| `model` | string | **Required.** Alias or `provider/model`. |
| `document_url` | string | Public http(s) URL, passed to the provider. |
| `document` | string | Base64 bytes (a `data:…;base64,` prefix is accepted). Needs `filename`. |
| `file` | multipart file part | The upload; its filename sets the type (or send `filename`). |
| `filename` | string | Sets the MIME type for `document` / overrides the part's filename. |
| `pages`, `language` | string | As in SPEC §4.1. |
| `output` | `"markdown"` \| `"text"` | Block content format (parse). |
| `output_format` | `"reducto"` \| `"extend"` \| `"llamaparse"` \| `"liteocr"` | Vendor-native response shape (parse, extract). |
| `provider_options` | object | Merged into the provider request verbatim. |
| `include_raw` | bool | Attach the provider payload as `raw`. |
| `timeout` | number (s) | Capped at `server.max_timeout_secs`. |
| `max_retries` | int | Per provider call (max 10). |
| `fallbacks` | string[] | Extra aliases/models tried after `model`'s own targets. |
| `metadata` | object | Echoed back in the response. |
| `schema`, `instructions`, `citations` | | `/v1/extract` only; `schema` required there. |

Exactly one of `document_url`, `document`, `file`. `api_key` and `base_url` are **rejected**: a
client must never be able to point the gateway's provider credentials at another host. Local file
paths are not accepted either.

The response is the unified `ParseResponse` / `TextResponse` / `ExtractResponse` JSON (or the
vendor shape), with headers `x-liteocr-model` (served model), `x-liteocr-cost-usd`, `x-request-id`.

### Errors

Every error has the same body:

```json
{"error": {"type": "provider_error", "message": "…the provider's own message…", "provider": "reducto",
           "provider_status": 503, "job_id": null, "request_id": "5c0f…"}}
```

| HTTP | `type` | Cause |
|---|---|---|
| 400 | `input_error`, `bad_request_error`, `unsupported_model_error` | Malformed request, provider 4xx, unknown model or model without this mode |
| 401 | `unauthorized` | Missing or unknown gateway key |
| 402 | `budget_exceeded` | Key's monthly budget spent |
| 403 | `model_not_allowed` | Model (or a fallback) not in the key's `models` |
| 413 | `payload_too_large` | Body over `max_body_mb` |
| 429 | `key_rate_limited` | Key's `rpm` reached (`Retry-After` set) |
| 429 | `rate_limit_error` | Provider rate limit after retries and fallbacks |
| 502 | `provider_error`, `network_error`, `authentication_error` | Provider 5xx / failed job, network failure, provider rejected the gateway's credentials |
| 504 | `timeout_error` | `timeout` exceeded |

Provider credential failures are 502, not 401: the caller's key was fine, the operator's was not.

### `GET /v1/models`

Aliases (targets, strategy, the modes every target supports) and, with `allow_direct_models`,
every registry model with its modes, per-page list price per mode and whether a provider key is
configured. Filtered to what the caller's key may use.

### `GET /v1/usage`

This month's spend, requests, pages, remaining budget and limits: the caller's own key, or all
keys for the master key.

### `GET /health`, `GET /metrics`

`/health` → `{"status": "ok", "version": …}`. `/metrics` is Prometheus text, unauthenticated
(it carries model names, counts and costs, no secrets or content; firewall it if that matters):

| Metric | Labels |
|---|---|
| `liteocr_requests_total` | `mode`, `model` (served model, or requested alias on failure, `-` if it never resolved), `status` |
| `liteocr_errors_total` | `type` (the error `type` above) |
| `liteocr_request_duration_seconds` (histogram, 0.25 s – 300 s buckets) | `mode` |
| `liteocr_pages_total` | `model` |
| `liteocr_cost_usd_total` | `model` |
| `liteocr_fallbacks_total` | — |

## Request log

One JSON object per request, on stdout and/or `log_file`:

```json
{"ts":"2026-09-24T20:23:52.140Z","request_id":"bf397168-…","key_id":"demo","method":"POST",
 "path":"/v1/parse","mode":"parse","model":"cheap","served_model":"llamaparse/cost_effective",
 "provider":"llamaparse","fallback_index":0,"pages":1,"cost_usd":0.00375,"latency_ms":10384,
 "status":200,"error_type":null,"provider_status":null}
```

The record has no field for document bytes, URLs, extracted content, provider error text (some
providers echo document text in errors), provider keys or gateway key secrets.

## curl examples

```bash
GW=http://127.0.0.1:4000; KEY=sk-billing-change-me

# Upload a file (multipart) to an alias
curl -s $GW/v1/parse -H "Authorization: Bearer $KEY" -F model=invoices -F file=@invoice.pdf | jq .markdown

# URL input, vendor-native shape, a request-level fallback
curl -s $GW/v1/parse -H "Authorization: Bearer $KEY" -H 'content-type: application/json' -d '{
  "model": "invoices", "document_url": "https://example.com/invoice.pdf",
  "output_format": "reducto", "fallbacks": ["llamaparse/cost_effective"]}'

# Base64 input, OCR mode
curl -s $GW/v1/ocr -H "x-api-key: $KEY" -H 'content-type: application/json' \
  -d "{\"model\": \"reducto\", \"filename\": \"scan.png\", \"document\": \"$(base64 -w0 scan.png)\"}" | jq .text

# Extract with a schema
curl -s $GW/v1/extract -H "Authorization: Bearer $KEY" -F model=invoice-fields -F file=@invoice.pdf \
  -F 'schema={"type":"object","properties":{"total":{"type":"number"}}}'

curl -s $GW/v1/models -H "Authorization: Bearer $KEY" | jq '.data[].id'
curl -s $GW/v1/usage  -H "Authorization: Bearer $KEY"
curl -s $GW/metrics
```

## Not in scope (yet)

Streaming or async job endpoints (calls are synchronous; long jobs hold the connection), key
management over HTTP (keys live in the config file; restart to change them), a database, response
caching, per-key budgets by model, TLS termination (put it behind a reverse proxy), and metrics
auth.
