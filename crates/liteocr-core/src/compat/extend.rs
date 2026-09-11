//! Render unified responses in Extend's native shape.
//!
//! Parse: the `parse_run` object returned by `GET /parse_runs/{id}` with
//! `chunkingStrategy={type:"page"}` — one `chunk` per page, block boxes in page pixels.
//! See `docs/providers/extend.md` §4 and `tests/fixtures/extend_parse_run.json`.

use super::{any_synthetic_dims, job_id, mean_confidence, min_confidence, num, page_dims};
use crate::types::{Block, BlockType, ExtractResponse, Page, ParseResponse};
use serde_json::{json, Value};

/// Unified block type → Extend's block vocabulary (`docs/providers/extend.md` §4).
///
/// Extend has no list or caption block, so those degrade to `text`.
pub(super) fn block_type(t: BlockType) -> &'static str {
    match t {
        BlockType::Title => "heading",
        BlockType::SectionHeader => "section_heading",
        BlockType::Table => "table",
        BlockType::Figure => "figure",
        BlockType::Formula => "formula",
        BlockType::Header => "header",
        BlockType::Footer => "footer",
        BlockType::Text | BlockType::List | BlockType::Footnote | BlockType::Caption | BlockType::Other => "text",
    }
}

/// `{left, top, right, bottom}` in page pixels plus the matching 4-point polygon.
fn geometry(b: &Block, w: f64, h: f64) -> (Value, Value) {
    match b.bbox {
        Some(bb) => {
            let (left, top, right, bottom) = (bb.x0 * w, bb.y0 * h, bb.x1 * w, bb.y1 * h);
            let polygon = json!([
                { "x": left, "y": top },
                { "x": right, "y": top },
                { "x": right, "y": bottom },
                { "x": left, "y": bottom },
            ]);
            (polygon, json!({ "left": left, "top": top, "right": right, "bottom": bottom }))
        }
        None => (json!([]), Value::Null),
    }
}

fn block(b: &Block, index: usize, page: &Page, w: f64, h: f64) -> Value {
    let (polygon, bounding_box) = geometry(b, w, h);
    json!({
        "object": "block",
        "id": format!("block_{}_{}", page.page_number, index + 1),
        "type": block_type(b.block_type),
        "content": b.content,
        "details": {},
        "metadata": {
            "page": { "number": b.page_number, "width": w, "height": h },
            "minOcrConfidence": num(b.confidence),
            "avgOcrConfidence": num(b.confidence),
        },
        "polygon": polygon,
        "boundingBox": bounding_box,
    })
}

fn chunk(page: &Page) -> Value {
    let (w, h, _) = page_dims(page);
    let blocks: Vec<Value> = page.blocks.iter().enumerate().map(|(i, b)| block(b, i, page, w, h)).collect();
    json!({
        "object": "chunk",
        "id": format!("chunk_{}", page.page_number),
        "type": "page",
        "content": page.markdown,
        "metadata": {
            "pageRange": { "start": page.page_number, "end": page.page_number },
            "minOcrConfidence": num(min_confidence(page.blocks.iter().map(|b| b.confidence))),
            "avgOcrConfidence": num(mean_confidence(page.blocks.iter().map(|b| b.confidence))),
        },
        "blocks": blocks,
    })
}

pub(super) fn render_parse(resp: &ParseResponse) -> Value {
    let chunks: Vec<Value> = resp.pages.iter().map(chunk).collect();
    let pages: Vec<Value> = resp
        .pages
        .iter()
        .map(|p| {
            let (w, h, _) = page_dims(p);
            json!({
                "number": p.page_number,
                "rotationApplied": 0,
                "originalPageWidth": w,
                "originalPageHeight": h,
                "dpi": null,
            })
        })
        .collect();
    // The run-level `metadata` map is free-form, so it is the one legal place to admit that the
    // page pixel sizes below were invented rather than reported by the provider.
    let metadata = if any_synthetic_dims(resp) { json!({ "liteocr_synthetic_page_dims": true }) } else { Value::Null };
    let credits = num(resp.usage.credits);
    json!({
        "object": "parse_run",
        "id": job_id(&resp.id, resp.provider_job_id.as_ref()),
        "file": null,
        "status": "PROCESSED",
        "failureReason": null,
        "failureMessage": null,
        "metadata": metadata,
        "dataRetention": null,
        "output": { "chunks": chunks, "metadata": { "pages": pages } },
        "outputUrl": null,
        "metrics": { "processingTimeMs": resp.latency_ms, "pageCount": resp.usage.pages },
        "config": { "target": "markdown", "chunkingStrategy": { "type": "page" }, "engine": null },
        "batchId": null,
        "usage": { "credits": credits, "totalCredits": credits, "breakdown": [] },
    })
}

// ---- extract (best effort) -----------------------------------------------------------------------

pub(super) fn render_extract(resp: &ExtractResponse) -> Value {
    let metadata: serde_json::Map<String, Value> = resp
        .fields
        .iter()
        .map(|(pointer, info)| {
            let citations: Vec<Value> = info
                .citations
                .iter()
                .map(|c| {
                    json!({
                        "pageNumber": c.page_number,
                        // No page dimensions survive an extract call, so these stay in the unified
                        // normalised 0..1 space (documented in COMPAT.md).
                        "boundingBox": c.bbox.map(|bb| json!({
                            "left": bb.x0, "top": bb.y0, "right": bb.x1, "bottom": bb.y1
                        })),
                        "content": c.text,
                    })
                })
                .collect();
            (
                pointer.trim_start_matches('/').to_string(),
                json!({ "confidence": num(info.confidence), "citations": citations }),
            )
        })
        .collect();
    let credits = num(resp.usage.credits);
    json!({
        "object": "extract_run",
        "id": job_id(&resp.id, resp.provider_job_id.as_ref()),
        "status": "PROCESSED",
        "output": { "value": resp.data, "metadata": metadata },
        "usage": { "credits": credits, "totalCredits": credits, "breakdown": [] },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BBox, Usage};

    fn page(width: Option<f64>, height: Option<f64>, blocks: Vec<Block>) -> Page {
        Page { page_number: 1, width, height, markdown: "# T".into(), text: "T".into(), blocks }
    }

    fn title_block() -> Block {
        Block {
            block_type: BlockType::Title,
            content: "# T".into(),
            text: None,
            bbox: Some(BBox { x0: 0.1, y0: 0.2, x1: 0.5, y1: 0.4 }),
            confidence: Some(0.9),
            page_number: 1,
        }
    }

    fn resp(pages: Vec<Page>) -> ParseResponse {
        ParseResponse::from_pages("x", "x/y", pages, Usage { pages: 1, credits: Some(4.0), provider_cost_usd: None })
    }

    #[test]
    fn boxes_are_scaled_into_page_pixels() {
        let v = render_parse(&resp(vec![page(Some(1000.0), Some(2000.0), vec![title_block()])]));
        let bb = &v["output"]["chunks"][0]["blocks"][0]["boundingBox"];
        assert_eq!(bb["left"], 100.0);
        assert_eq!(bb["top"], 400.0);
        assert_eq!(bb["right"], 500.0);
        assert_eq!(bb["bottom"], 800.0);
        assert_eq!(v["output"]["chunks"][0]["blocks"][0]["polygon"].as_array().unwrap().len(), 4);
        assert_eq!(v["metadata"], Value::Null, "real page dims must not be flagged synthetic");
        assert_eq!(v["output"]["metadata"]["pages"][0]["originalPageWidth"], 1000.0);
    }

    #[test]
    fn unknown_page_dims_are_flagged_and_default_to_1000() {
        let v = render_parse(&resp(vec![page(None, None, vec![title_block()])]));
        assert_eq!(v["metadata"]["liteocr_synthetic_page_dims"], true);
        assert_eq!(v["output"]["chunks"][0]["blocks"][0]["metadata"]["page"]["width"], 1000.0);
        assert_eq!(v["output"]["chunks"][0]["blocks"][0]["boundingBox"]["right"], 500.0);
    }

    #[test]
    fn blockless_and_boxless_pages_render() {
        let bare = Block {
            block_type: BlockType::Other,
            content: "x".into(),
            text: None,
            bbox: None,
            confidence: None,
            page_number: 1,
        };
        let v = render_parse(&resp(vec![page(None, None, vec![bare])]));
        let blk = &v["output"]["chunks"][0]["blocks"][0];
        assert_eq!(blk["boundingBox"], Value::Null);
        assert_eq!(blk["polygon"], json!([]));
        assert_eq!(blk["type"], "text");
        assert_eq!(v["output"]["chunks"][0]["metadata"]["avgOcrConfidence"], Value::Null);
    }

    #[test]
    fn envelope_is_a_processed_parse_run() {
        let mut r = resp(vec![page(Some(10.0), Some(10.0), vec![title_block()])]);
        r.latency_ms = 4383;
        r.provider_job_id = Some("pr_abc".into());
        let v = render_parse(&r);
        assert_eq!(v["object"], "parse_run");
        assert_eq!(v["id"], "pr_abc");
        assert_eq!(v["status"], "PROCESSED");
        assert_eq!(v["metrics"]["processingTimeMs"], 4383);
        assert_eq!(v["metrics"]["pageCount"], 1);
        assert_eq!(v["usage"]["totalCredits"], 4.0);
        assert_eq!(v["config"]["chunkingStrategy"]["type"], "page");
    }
}
