//! Render unified responses in Reducto's native shape.
//!
//! Parse: the `POST /parse` response object (`response_type: "parse"`) with `chunk_mode=page`
//! semantics — one chunk per page, blocks carrying already-normalised 0..1 boxes.
//! See `docs/providers/reducto.md` §4 and `tests/fixtures/reducto_parse.json`.

use super::{job_id, num};
use crate::types::{Block, BlockType, Citation, ExtractResponse, FieldInfo, ParseResponse};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Unified block type → Reducto's block vocabulary.
///
/// Reducto's vocabulary is a closed set of title-cased strings. `footnote`, `caption`, `formula`
/// and `other` have no stable counterpart, so they degrade to `Text` (the type a Reducto consumer
/// treats as "prose with content"); see `docs/COMPAT.md` for why that is lossy.
pub(super) fn block_type(t: BlockType) -> &'static str {
    match t {
        BlockType::Title => "Title",
        BlockType::SectionHeader => "Section Header",
        BlockType::Text => "Text",
        BlockType::List => "List Item",
        BlockType::Table => "Table",
        BlockType::Figure => "Figure",
        BlockType::Header => "Header",
        BlockType::Footer => "Footer",
        BlockType::Footnote | BlockType::Caption | BlockType::Formula | BlockType::Other => "Text",
    }
}

/// Reducto's coarse label: `high` at or above 0.8, `low` below it, `null` when unknown.
fn coarse_confidence(c: Option<f64>) -> Value {
    match c {
        Some(c) if c >= 0.8 => Value::from("high"),
        Some(_) => Value::from("low"),
        None => Value::Null,
    }
}

fn bbox(b: &Block) -> Value {
    let page = b.page_number;
    match b.bbox {
        // Reducto boxes are `left/top/width/height`, already normalised 0..1, origin top-left —
        // exactly the unified convention, so this is a pure re-shaping.
        Some(bb) => json!({
            "left": bb.x0,
            "top": bb.y0,
            "width": bb.x1 - bb.x0,
            "height": bb.y1 - bb.y0,
            "page": page,
            "original_page": page,
        }),
        None => json!({ "left": 0.0, "top": 0.0, "width": 0.0, "height": 0.0, "page": page, "original_page": page }),
    }
}

fn block(b: &Block) -> Value {
    json!({
        "type": block_type(b.block_type),
        "bbox": bbox(b),
        "content": b.content,
        "image_url": null,
        "chart_data": null,
        "confidence": coarse_confidence(b.confidence),
        "granular_confidence": { "extract_confidence": null, "parse_confidence": num(b.confidence) },
        "extra": null,
    })
}

pub(super) fn render_parse(resp: &ParseResponse) -> Value {
    let chunks: Vec<Value> = resp
        .pages
        .iter()
        .map(|p| {
            json!({
                "content": p.markdown,
                "embed": p.markdown,
                "enriched": null,
                "enrichment_success": false,
                "blocks": p.blocks.iter().map(block).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "response_type": "parse",
        "job_id": job_id(&resp.id, resp.provider_job_id.as_ref()),
        "duration": resp.latency_ms as f64 / 1000.0,
        "pdf_url": null,
        "studio_link": null,
        "usage": {
            "num_pages": resp.usage.pages,
            "credits": num(resp.usage.credits),
            "credit_breakdown": null,
            "page_billing_breakdown": null,
            "non_empty_cell_count": null,
        },
        "result": { "type": "full", "chunks": chunks, "ocr": null, "custom": null },
        "parse_mode": null,
        "document_properties": null,
    })
}

// ---- extract (best effort) -----------------------------------------------------------------------

/// Append one unescaped segment to a JSON pointer (RFC 6901).
fn push_pointer(base: &str, segment: &str) -> String {
    format!("{base}/{}", segment.replace('~', "~0").replace('/', "~1"))
}

/// A citation is rendered as the parse block Reducto would have cited.
fn citation(c: &Citation, info: &FieldInfo) -> Value {
    let bb = c.bbox.map(|bb| {
        json!({
            "left": bb.x0,
            "top": bb.y0,
            "width": bb.x1 - bb.x0,
            "height": bb.y1 - bb.y0,
            "page": c.page_number,
            "original_page": c.page_number,
        })
    });
    json!({
        "type": "Text",
        "bbox": bb,
        "content": c.text.clone().unwrap_or_default(),
        "confidence": coarse_confidence(info.confidence),
        "granular_confidence": { "extract_confidence": num(info.confidence), "parse_confidence": null },
    })
}

/// Rebuild Reducto's `{value, citations}` leaf wrappers from the unified pointer-keyed `fields`.
/// This is the inverse of the unwrapping the provider does when normalising.
fn rewrap(node: &Value, pointer: &str, fields: &BTreeMap<String, FieldInfo>) -> Value {
    if let Some(info) = fields.get(pointer) {
        let citations: Vec<Value> = info.citations.iter().map(|c| citation(c, info)).collect();
        return json!({ "value": rewrap_children(node, pointer, fields), "citations": citations });
    }
    rewrap_children(node, pointer, fields)
}

fn rewrap_children(node: &Value, pointer: &str, fields: &BTreeMap<String, FieldInfo>) -> Value {
    match node {
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), rewrap(v, &push_pointer(pointer, k), fields))).collect())
        }
        Value::Array(items) => Value::Array(
            items.iter().enumerate().map(|(i, v)| rewrap(v, &push_pointer(pointer, &i.to_string()), fields)).collect(),
        ),
        other => other.clone(),
    }
}

pub(super) fn render_extract(resp: &ExtractResponse) -> Value {
    let num_fields = resp
        .metadata
        .get("reducto_num_fields")
        .and_then(Value::as_u64)
        .or_else(|| resp.data.as_object().map(|o| o.len() as u64));
    json!({
        "response_type": "v3_extract",
        "job_id": job_id(&resp.id, resp.provider_job_id.as_ref()),
        "usage": {
            "num_pages": resp.usage.pages,
            "num_fields": num_fields,
            "credits": num(resp.usage.credits),
        },
        "result": rewrap(&resp.data, "", &resp.fields),
        "studio_link": null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BBox, Usage};

    fn resp_with(blocks: Vec<Block>) -> ParseResponse {
        let pages = crate::types::pages_from_blocks(blocks, &Default::default());
        ParseResponse::from_pages("x", "x/y", pages, Usage { pages: 1, credits: Some(2.0), provider_cost_usd: None })
    }

    #[test]
    fn coarse_confidence_threshold_is_inclusive() {
        assert_eq!(coarse_confidence(Some(0.8)), "high");
        assert_eq!(coarse_confidence(Some(0.7999)), "low");
        assert_eq!(coarse_confidence(None), Value::Null);
    }

    #[test]
    fn missing_bbox_renders_zero_box_on_the_block_page() {
        let b = Block {
            block_type: BlockType::Figure,
            content: "x".into(),
            text: None,
            bbox: None,
            confidence: None,
            page_number: 3,
        };
        let v = super::bbox(&b);
        assert_eq!(v, json!({"left":0.0,"top":0.0,"width":0.0,"height":0.0,"page":3,"original_page":3}));
    }

    #[test]
    fn block_types_degrade_to_text() {
        for t in [BlockType::Footnote, BlockType::Caption, BlockType::Formula, BlockType::Other] {
            assert_eq!(block_type(t), "Text", "{t:?} should degrade to Text");
        }
        assert_eq!(block_type(BlockType::SectionHeader), "Section Header");
    }

    #[test]
    fn parse_envelope_is_reducto_shaped() {
        let b = Block {
            block_type: BlockType::Title,
            content: "# T".into(),
            text: None,
            bbox: Some(BBox { x0: 0.1, y0: 0.2, x1: 0.4, y1: 0.5 }),
            confidence: Some(0.95),
            page_number: 1,
        };
        let mut resp = resp_with(vec![b]);
        resp.latency_ms = 1500;
        let v = render_parse(&resp);
        assert_eq!(v["response_type"], "parse");
        assert_eq!(v["duration"], 1.5);
        assert_eq!(v["usage"]["num_pages"], 1);
        assert_eq!(v["usage"]["credits"], 2.0);
        let blk = &v["result"]["chunks"][0]["blocks"][0];
        assert_eq!(blk["type"], "Title");
        assert_eq!(blk["confidence"], "high");
        assert_eq!(blk["granular_confidence"]["parse_confidence"], 0.95);
        assert!((blk["bbox"]["width"].as_f64().unwrap() - 0.3).abs() < 1e-12);
        assert_eq!(v["result"]["chunks"][0]["embed"], v["result"]["chunks"][0]["content"]);
    }

    #[test]
    fn extract_rewraps_citation_leaves() {
        let mut resp = ExtractResponse::new(
            "x",
            "x/y",
            json!({"invoice": {"total": "56.78"}}),
            Usage { pages: 1, credits: None, provider_cost_usd: None },
        );
        resp.fields.insert(
            "/invoice/total".into(),
            FieldInfo {
                confidence: Some(0.91),
                citations: vec![Citation {
                    page_number: 2,
                    bbox: Some(BBox { x0: 0.1, y0: 0.1, x1: 0.2, y1: 0.2 }),
                    text: Some("Total: $56.78".into()),
                }],
            },
        );
        let v = render_extract(&resp);
        assert_eq!(v["response_type"], "v3_extract");
        assert_eq!(v["usage"]["num_fields"], 1);
        let leaf = &v["result"]["invoice"]["total"];
        assert_eq!(leaf["value"], "56.78");
        assert_eq!(leaf["citations"][0]["bbox"]["page"], 2);
        assert_eq!(leaf["citations"][0]["content"], "Total: $56.78");
        assert_eq!(leaf["citations"][0]["granular_confidence"]["extract_confidence"], 0.91);
        // Fields without metadata stay bare values.
        assert!(v["result"]["invoice"].get("value").is_none());
    }
}
