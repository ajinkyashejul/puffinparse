//! Render unified responses in LlamaParse's native shape.
//!
//! Parse: the `GET /api/v1/parsing/job/{id}/result/json` payload — `pages[]` with `md`, `text` and
//! `items[]` whose `bBox` is in page units.
//! See `docs/providers/llamaparse.md` §4 and `tests/fixtures/llamaparse_result_json.json`.

use super::{job_id, mean_confidence, num, page_dims};
use crate::types::{Block, BlockType, ExtractResponse, Page, ParseResponse};
use serde_json::{json, Value};

/// Unified block type → LlamaParse's item vocabulary (`heading` / `text` / `table`).
///
/// LlamaParse keeps figures in `images[]` / `charts[]` rather than `items[]`, and has no list,
/// header, footer or formula item type, so everything outside the three item types becomes `text`
/// carrying the block's markdown.
pub(super) fn item_type(t: BlockType) -> &'static str {
    match t {
        BlockType::Title | BlockType::SectionHeader => "heading",
        BlockType::Table => "table",
        _ => "text",
    }
}

/// Heading level: 1 for a document title, 2 for a section header, absent for everything else.
fn level(t: BlockType) -> Value {
    match t {
        BlockType::Title => Value::from(1),
        BlockType::SectionHeader => Value::from(2),
        _ => Value::Null,
    }
}

fn item(b: &Block, w: f64, h: f64) -> Value {
    let bbox = b.bbox.map(|bb| {
        json!({
            "x": bb.x0 * w,
            "y": bb.y0 * h,
            "w": (bb.x1 - bb.x0) * w,
            "h": (bb.y1 - bb.y0) * h,
            "confidence": num(b.confidence),
        })
    });
    json!({
        "type": item_type(b.block_type),
        "md": b.content,
        "value": b.text,
        "lvl": level(b.block_type),
        "bBox": bbox,
        "layoutAwareBbox": [],
    })
}

fn page(p: &Page) -> Value {
    let (w, h, _) = page_dims(p);
    let items: Vec<Value> = p.blocks.iter().map(|b| item(b, w, h)).collect();
    json!({
        "page": p.page_number,
        "text": p.text,
        "md": p.markdown,
        "images": [],
        "charts": [],
        "items": items,
        "status": "OK",
        "width": w,
        "height": h,
        "links": [],
        "triggeredAutoMode": false,
        "parsingMode": null,
        "structuredData": null,
        "noStructuredContent": false,
        "noTextContent": false,
        "pageHeaderMarkdown": "",
        "pageFooterMarkdown": "",
        "printedPageNumber": "",
        "confidence": num(mean_confidence(p.blocks.iter().map(|b| b.confidence))),
    })
}

pub(super) fn render_parse(resp: &ParseResponse) -> Value {
    let credits = resp.usage.credits.unwrap_or(0.0);
    json!({
        "pages": resp.pages.iter().map(page).collect::<Vec<_>>(),
        "job_metadata": {
            "credits_used": credits,
            "job_credits_usage": credits,
            "job_pages": resp.usage.pages,
            "job_auto_mode_triggered_pages": 0,
            "job_is_cache_hit": resp.metadata.get("llamaparse_cache_hit").and_then(Value::as_bool).unwrap_or(false),
        },
    })
}

// ---- extract (best effort) -----------------------------------------------------------------------

/// LlamaExtract's `{data, extraction_metadata}` envelope. LlamaExtract reports per-field
/// confidence and citations under `extraction_metadata.field_metadata`, keyed by field path.
pub(super) fn render_extract(resp: &ExtractResponse) -> Value {
    let field_metadata: serde_json::Map<String, Value> = resp
        .fields
        .iter()
        .map(|(pointer, info)| {
            let citations: Vec<Value> = info
                .citations
                .iter()
                .map(|c| {
                    json!({
                        "page": c.page_number,
                        // Normalised 0..1: an extract response carries no page dimensions.
                        "bBox": c.bbox.map(|bb| json!({
                            "x": bb.x0, "y": bb.y0, "w": bb.x1 - bb.x0, "h": bb.y1 - bb.y0
                        })),
                        "text": c.text,
                    })
                })
                .collect();
            (
                pointer.trim_start_matches('/').replace('/', "."),
                json!({ "confidence": num(info.confidence), "citations": citations }),
            )
        })
        .collect();
    json!({
        "data": resp.data,
        "extraction_metadata": { "field_metadata": field_metadata, "job_id": job_id(&resp.id, resp.provider_job_id.as_ref()) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BBox, Usage};

    fn block(t: BlockType, bbox: Option<BBox>) -> Block {
        Block {
            block_type: t,
            content: "# T".into(),
            text: Some("T".into()),
            bbox,
            confidence: Some(0.8),
            page_number: 1,
        }
    }

    fn resp(width: Option<f64>, height: Option<f64>, blocks: Vec<Block>) -> ParseResponse {
        let p = Page { page_number: 1, width, height, markdown: "# T".into(), text: "T".into(), blocks };
        ParseResponse::from_pages("x", "x/y", vec![p], Usage { pages: 1, credits: None, provider_cost_usd: None })
    }

    #[test]
    fn heading_levels_follow_the_unified_type() {
        let v = render_parse(&resp(
            Some(1000.0),
            Some(1300.0),
            vec![
                block(BlockType::Title, None),
                block(BlockType::SectionHeader, None),
                block(BlockType::Text, None),
                block(BlockType::List, None),
            ],
        ));
        let items = v["pages"][0]["items"].as_array().unwrap();
        assert_eq!((items[0]["type"].as_str(), items[0]["lvl"].as_u64()), (Some("heading"), Some(1)));
        assert_eq!((items[1]["type"].as_str(), items[1]["lvl"].as_u64()), (Some("heading"), Some(2)));
        assert_eq!(items[2]["type"], "text");
        assert_eq!(items[2]["lvl"], Value::Null);
        // Lists have no LlamaParse item type and arrive as text carrying their markdown.
        assert_eq!(items[3]["type"], "text");
        assert_eq!(items[3]["md"], "# T");
    }

    #[test]
    fn boxes_are_in_page_units_with_a_1000_square_fallback() {
        let bb = BBox { x0: 0.1, y0: 0.2, x1: 0.3, y1: 0.4 };
        let v = render_parse(&resp(Some(1000.0), Some(1300.0), vec![block(BlockType::Text, Some(bb))]));
        let b = &v["pages"][0]["items"][0]["bBox"];
        assert_eq!(b["x"], 100.0);
        assert_eq!(b["y"], 260.0);
        assert!((b["w"].as_f64().unwrap() - 200.0).abs() < 1e-9);
        assert_eq!(b["confidence"], 0.8);

        let v = render_parse(&resp(None, None, vec![block(BlockType::Text, Some(bb))]));
        assert_eq!(v["pages"][0]["width"], 1000.0);
        assert_eq!(v["pages"][0]["height"], 1000.0);
        assert_eq!(v["pages"][0]["items"][0]["bBox"]["y"], 200.0);
    }

    #[test]
    fn missing_geometry_and_credits_render_as_null_and_zero() {
        let mut r = resp(None, None, vec![block(BlockType::Text, None)]);
        r.pages[0].blocks[0].confidence = None;
        let v = render_parse(&r);
        assert_eq!(v["pages"][0]["items"][0]["bBox"], Value::Null);
        assert_eq!(v["pages"][0]["confidence"], Value::Null);
        assert_eq!(v["job_metadata"]["credits_used"], 0.0);
        assert_eq!(v["job_metadata"]["job_pages"], 1);
        assert_eq!(v["job_metadata"]["job_is_cache_hit"], false);
    }

    #[test]
    fn extract_envelope_has_field_metadata() {
        let mut r = ExtractResponse::new(
            "x",
            "x/y",
            json!({"total": "56.78"}),
            Usage { pages: 1, credits: None, provider_cost_usd: None },
        );
        r.fields.insert(
            "/total".into(),
            crate::types::FieldInfo {
                confidence: Some(0.7),
                citations: vec![crate::types::Citation { page_number: 1, bbox: None, text: Some("t".into()) }],
            },
        );
        let v = render_extract(&r);
        assert_eq!(v["data"]["total"], "56.78");
        assert_eq!(v["extraction_metadata"]["field_metadata"]["total"]["confidence"], 0.7);
        assert_eq!(v["extraction_metadata"]["field_metadata"]["total"]["citations"][0]["page"], 1);
    }
}
