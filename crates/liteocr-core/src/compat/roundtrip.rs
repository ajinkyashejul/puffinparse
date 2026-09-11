//! Round-trip validation of the native-format renderers against the real provider fixtures.
//!
//! The contract this file enforces, for every provider X:
//!
//! ```text
//! X fixture --X::normalize--> ParseResponse --render_parse(X)--> JSON ≅ X fixture
//! ```
//!
//! `≅` is *skeleton* equality, not byte equality: same top-level key set (after the keys COMPAT.md
//! documents as always-null are accounted for), same number of chunks/pages and blocks/items,
//! identical content strings, block types equal up to the provider's own type mapping, boxes equal
//! within 1e-6 once both sides are converted to the unified 0..1 space, and the same billed page
//! count. [`skeleton_diff`] reports the first mismatch with a path.
//!
//! These tests live inside the crate because the provider `normalize` functions are `pub(crate)`.

use super::{render_parse, Format};
use crate::types::{BlockType, OutputFormat, ParseResponse};
use serde_json::Value;
use std::collections::BTreeSet;

const REDUCTO_FIXTURE: &str = include_str!("../../tests/fixtures/reducto_parse.json");
const EXTEND_FIXTURE: &str = include_str!("../../tests/fixtures/extend_parse_run.json");
const LLAMAPARSE_FIXTURE: &str = include_str!("../../tests/fixtures/llamaparse_result_json.json");

/// Boxes agree to this tolerance after being mapped back to the unified 0..1 space.
const BBOX_EPS: f64 = 1e-6;

// ---- provider forward maps -----------------------------------------------------------------------
//
// Mirrors of the `map_block_type` functions in `providers/*.rs` (which are private). Having them
// here means the round-trip compares block types *up to the provider's own mapping*: `key_value`
// and `text` are both `text` to Extend's reader, so a `key_value` block that comes back as `text`
// is a faithful — if lossy — render, while a `heading` that came back as `table` is a bug.

fn reducto_block_type(t: &str) -> BlockType {
    match t {
        "Text" | "Key Value" | "Comment" => BlockType::Text,
        "Title" => BlockType::Title,
        "Section Header" => BlockType::SectionHeader,
        "List Item" => BlockType::List,
        "Table" => BlockType::Table,
        "Figure" => BlockType::Figure,
        "Header" => BlockType::Header,
        "Footer" => BlockType::Footer,
        "Footnote" => BlockType::Footnote,
        "Caption" => BlockType::Caption,
        "Formula" | "Equation" => BlockType::Formula,
        _ => BlockType::Other,
    }
}

fn extend_block_type(t: &str) -> BlockType {
    match t {
        "text" | "key_value" => BlockType::Text,
        "heading" => BlockType::Title,
        "section_heading" => BlockType::SectionHeader,
        "table" | "table_head" | "table_cell" => BlockType::Table,
        "figure" => BlockType::Figure,
        "formula" => BlockType::Formula,
        "header" => BlockType::Header,
        "footer" => BlockType::Footer,
        _ => BlockType::Other,
    }
}

fn llamaparse_block_type(t: &str, lvl: Option<u64>) -> BlockType {
    match t {
        "heading" => {
            if lvl == Some(1) {
                BlockType::Title
            } else {
                BlockType::SectionHeader
            }
        }
        "text" => BlockType::Text,
        "table" => BlockType::Table,
        "list" | "list_item" => BlockType::List,
        "figure" | "image" | "chart" => BlockType::Figure,
        "formula" | "equation" => BlockType::Formula,
        "header" => BlockType::Header,
        "footer" => BlockType::Footer,
        _ => BlockType::Other,
    }
}

// ---- skeletons -----------------------------------------------------------------------------------

/// One block / item, in units that are comparable across vendors.
#[derive(Debug, Clone, PartialEq)]
struct Blk {
    ty: BlockType,
    content: String,
    /// `[x0, y0, x1, y1]` normalised to 0..1, or `None` when the vendor reported no geometry.
    bbox: Option<[f64; 4]>,
}

/// One chunk / page.
#[derive(Debug, Clone)]
struct Unit {
    content: String,
    blocks: Vec<Blk>,
}

#[derive(Debug, Clone)]
struct Skeleton {
    top_keys: BTreeSet<String>,
    unit_keys: BTreeSet<String>,
    block_keys: BTreeSet<String>,
    usage_pages: Option<u64>,
    units: Vec<Unit>,
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

/// Union of the keys of every element, so an optional key present on only some blocks still counts.
fn element_keys(items: &[Value]) -> BTreeSet<String> {
    items.iter().flat_map(keys).collect()
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn array<'a>(v: &'a Value, path: &str) -> &'a Vec<Value> {
    static EMPTY: std::sync::OnceLock<Vec<Value>> = std::sync::OnceLock::new();
    v.pointer(path).and_then(Value::as_array).unwrap_or_else(|| EMPTY.get_or_init(Vec::new))
}

/// Pull the comparable skeleton out of a native payload (a real fixture or one we rendered).
fn skeleton(v: &Value, format: Format) -> Skeleton {
    match format {
        Format::Reducto => {
            let chunks = array(v, "/result/chunks");
            let units = chunks
                .iter()
                .map(|c| Unit {
                    content: c["content"].as_str().unwrap_or_default().trim().to_string(),
                    blocks: array(c, "/blocks")
                        .iter()
                        .map(|b| Blk {
                            ty: reducto_block_type(b["type"].as_str().unwrap_or_default()),
                            content: b["content"].as_str().unwrap_or_default().trim().to_string(),
                            // Reducto boxes are already normalised 0..1.
                            bbox: b.get("bbox").filter(|x| !x.is_null()).map(|bb| {
                                [
                                    f(&bb["left"]),
                                    f(&bb["top"]),
                                    f(&bb["left"]) + f(&bb["width"]),
                                    f(&bb["top"]) + f(&bb["height"]),
                                ]
                            }),
                        })
                        .collect(),
                })
                .collect::<Vec<_>>();
            Skeleton {
                top_keys: keys(v),
                unit_keys: element_keys(chunks),
                block_keys: element_keys(&chunks.iter().flat_map(|c| array(c, "/blocks").clone()).collect::<Vec<_>>()),
                usage_pages: v.pointer("/usage/num_pages").and_then(Value::as_u64),
                units,
            }
        }
        Format::Extend => {
            let chunks = array(v, "/output/chunks");
            let units = chunks
                .iter()
                .map(|c| Unit {
                    content: c["content"].as_str().unwrap_or_default().trim().to_string(),
                    blocks: array(c, "/blocks")
                        .iter()
                        .map(|b| {
                            let (pw, ph) = (
                                b.pointer("/metadata/page/width").and_then(Value::as_f64).unwrap_or(0.0),
                                b.pointer("/metadata/page/height").and_then(Value::as_f64).unwrap_or(0.0),
                            );
                            Blk {
                                ty: extend_block_type(b["type"].as_str().unwrap_or_default()),
                                content: b["content"].as_str().unwrap_or_default().trim().to_string(),
                                // Page pixels → unified 0..1.
                                bbox: b
                                    .get("boundingBox")
                                    .filter(|x| !x.is_null())
                                    .filter(|_| pw > 0.0 && ph > 0.0)
                                    .map(|bb| {
                                        [
                                            f(&bb["left"]) / pw,
                                            f(&bb["top"]) / ph,
                                            f(&bb["right"]) / pw,
                                            f(&bb["bottom"]) / ph,
                                        ]
                                    }),
                            }
                        })
                        .collect(),
                })
                .collect::<Vec<_>>();
            Skeleton {
                top_keys: keys(v),
                unit_keys: element_keys(chunks),
                block_keys: element_keys(&chunks.iter().flat_map(|c| array(c, "/blocks").clone()).collect::<Vec<_>>()),
                usage_pages: v.pointer("/metrics/pageCount").and_then(Value::as_u64),
                units,
            }
        }
        Format::LlamaParse => {
            let pages = array(v, "/pages");
            let units = pages
                .iter()
                .map(|p| {
                    let (pw, ph) = (f(&p["width"]), f(&p["height"]));
                    Unit {
                        content: p["md"].as_str().unwrap_or_default().trim().to_string(),
                        blocks: array(p, "/items")
                            .iter()
                            .map(|it| Blk {
                                ty: llamaparse_block_type(
                                    it["type"].as_str().unwrap_or_default(),
                                    it.get("lvl").and_then(Value::as_u64),
                                ),
                                content: it["md"].as_str().unwrap_or_default().trim().to_string(),
                                // Page units → unified 0..1.
                                bbox: it.get("bBox").filter(|x| !x.is_null()).filter(|_| pw > 0.0 && ph > 0.0).map(
                                    |bb| {
                                        [
                                            f(&bb["x"]) / pw,
                                            f(&bb["y"]) / ph,
                                            (f(&bb["x"]) + f(&bb["w"])) / pw,
                                            (f(&bb["y"]) + f(&bb["h"])) / ph,
                                        ]
                                    },
                                ),
                            })
                            .collect(),
                    }
                })
                .collect::<Vec<_>>();
            Skeleton {
                top_keys: keys(v),
                unit_keys: element_keys(pages),
                block_keys: element_keys(&pages.iter().flat_map(|p| array(p, "/items").clone()).collect::<Vec<_>>()),
                usage_pages: v.pointer("/job_metadata/job_pages").and_then(Value::as_u64),
                units,
            }
        }
        Format::Liteocr => Skeleton {
            top_keys: keys(v),
            unit_keys: element_keys(array(v, "/pages")),
            block_keys: element_keys(
                &array(v, "/pages").iter().flat_map(|p| array(p, "/blocks").clone()).collect::<Vec<_>>(),
            ),
            usage_pages: v.pointer("/usage/pages").and_then(Value::as_u64),
            units: array(v, "/pages")
                .iter()
                .map(|p| Unit {
                    content: p["markdown"].as_str().unwrap_or_default().trim().to_string(),
                    blocks: vec![],
                })
                .collect(),
        },
    }
}

/// Keys a fixture has that LiteOCR never populates, per format. Every one of these is listed in
/// `docs/COMPAT.md` §3 as "absent".
fn absent_keys(format: Format) -> &'static [&'static str] {
    match format {
        // Per-page extras from add-ons LiteOCR does not model (layout blocks, slide notes, page
        // screenshots), a raw orientation field with no unified home, and the table-item extras
        // (`csv`/`html`/`rows`/`isPerfectTable`) that restate a table the unified `content`
        // already carries as markdown.
        Format::LlamaParse => &[
            "originalOrientationAngle",
            "slideSpeakerNotes",
            "slideSectionName",
            "layout",
            "costOptimized",
            "csv",
            "html",
            "rows",
            "isPerfectTable",
        ],
        _ => &[],
    }
}

/// The first structural difference between `expected` (a real vendor payload) and `actual` (what we
/// rendered), or `Ok(())`. `types` turns the block-type comparison on; cross-format checks leave it
/// off because vendors do not share a block vocabulary.
fn skeleton_diff(expected: &Value, actual: &Value, format: Format, types: bool) -> Result<(), String> {
    let (e, a) = (skeleton(expected, format), skeleton(actual, format));

    if e.top_keys != a.top_keys {
        let missing: Vec<_> = e.top_keys.difference(&a.top_keys).collect();
        let extra: Vec<_> = a.top_keys.difference(&e.top_keys).collect();
        return Err(format!("top-level keys differ: missing {missing:?}, unexpected {extra:?}"));
    }

    if e.usage_pages != a.usage_pages {
        return Err(format!("billed page count differs: expected {:?}, got {:?}", e.usage_pages, a.usage_pages));
    }

    if e.units.len() != a.units.len() {
        return Err(format!("chunk/page count differs: expected {}, got {}", e.units.len(), a.units.len()));
    }

    // Key sets are unions over the elements, so they only mean anything once the counts agree.
    let ignore: BTreeSet<String> = absent_keys(format).iter().map(|s| s.to_string()).collect();
    let missing_unit: Vec<_> = e.unit_keys.difference(&a.unit_keys).filter(|k| !ignore.contains(*k)).collect();
    if !missing_unit.is_empty() {
        return Err(format!("chunk/page object is missing keys {missing_unit:?}"));
    }
    let missing_block: Vec<_> = e.block_keys.difference(&a.block_keys).filter(|k| !ignore.contains(*k)).collect();
    if !missing_block.is_empty() {
        return Err(format!("block/item object is missing keys {missing_block:?}"));
    }

    for (i, (eu, au)) in e.units.iter().zip(&a.units).enumerate() {
        if eu.content != au.content {
            return Err(format!(
                "unit[{i}].content differs:\n  expected {:?}\n  got      {:?}",
                eu.content, au.content
            ));
        }
        if eu.blocks.len() != au.blocks.len() {
            return Err(format!(
                "unit[{i}] block count differs: expected {}, got {}",
                eu.blocks.len(),
                au.blocks.len()
            ));
        }
        for (j, (eb, ab)) in eu.blocks.iter().zip(&au.blocks).enumerate() {
            if eb.content != ab.content {
                return Err(format!(
                    "unit[{i}].block[{j}].content differs:\n  expected {:?}\n  got      {:?}",
                    eb.content, ab.content
                ));
            }
            if types && eb.ty != ab.ty {
                return Err(format!("unit[{i}].block[{j}].type differs: expected {:?}, got {:?}", eb.ty, ab.ty));
            }
            match (eb.bbox, ab.bbox) {
                (None, None) => {}
                (Some(x), Some(y)) => {
                    if x.iter().zip(&y).any(|(p, q)| (p - q).abs() > BBOX_EPS) {
                        return Err(format!(
                            "unit[{i}].block[{j}].bbox differs beyond {BBOX_EPS}:\n  expected {x:?}\n  got      {y:?}"
                        ));
                    }
                }
                (x, y) => return Err(format!("unit[{i}].block[{j}].bbox presence differs: expected {x:?}, got {y:?}")),
            }
        }
    }
    Ok(())
}

// ---- fixtures → unified --------------------------------------------------------------------------

fn unified_reducto() -> ParseResponse {
    use crate::providers::reducto::{self, ParseResult};
    let parsed: reducto::WireParseResponse = serde_json::from_str(REDUCTO_FIXTURE).unwrap();
    let ParseResult::Full(full) = &parsed.result else { panic!("fixture must be a full result") };
    reducto::normalize(&parsed, full, OutputFormat::Markdown, "standard")
}

fn unified_extend() -> ParseResponse {
    use crate::providers::extend;
    let run: extend::ParseRun = serde_json::from_str(EXTEND_FIXTURE).unwrap();
    let output = run.output.clone().unwrap();
    extend::normalize(&run, output, OutputFormat::Markdown)
}

fn unified_llamaparse() -> ParseResponse {
    use crate::providers::llamaparse;
    let result: llamaparse::JsonResult = serde_json::from_str(LLAMAPARSE_FIXTURE).unwrap();
    llamaparse::normalize(&result, OutputFormat::Markdown, "agentic")
}

fn fixture(format: Format) -> Value {
    let raw = match format {
        Format::Reducto => REDUCTO_FIXTURE,
        Format::Extend => EXTEND_FIXTURE,
        Format::LlamaParse => LLAMAPARSE_FIXTURE,
        Format::Liteocr => unreachable!("no native fixture for the unified format"),
    };
    serde_json::from_str(raw).unwrap()
}

fn unified(format: Format) -> ParseResponse {
    match format {
        Format::Reducto => unified_reducto(),
        Format::Extend => unified_extend(),
        Format::LlamaParse => unified_llamaparse(),
        Format::Liteocr => unreachable!(),
    }
}

const NATIVE: &[Format] = &[Format::Reducto, Format::Extend, Format::LlamaParse];

// ---- self round-trips ----------------------------------------------------------------------------

#[test]
fn reducto_fixture_round_trips_through_reducto_format() {
    let rendered = render_parse(&unified_reducto(), Format::Reducto);
    if let Err(e) = skeleton_diff(&fixture(Format::Reducto), &rendered, Format::Reducto, true) {
        panic!("reducto self round-trip: {e}");
    }
}

#[test]
fn extend_fixture_round_trips_through_extend_format() {
    let rendered = render_parse(&unified_extend(), Format::Extend);
    if let Err(e) = skeleton_diff(&fixture(Format::Extend), &rendered, Format::Extend, true) {
        panic!("extend self round-trip: {e}");
    }
}

#[test]
fn llamaparse_fixture_round_trips_through_llamaparse_format() {
    let rendered = render_parse(&unified_llamaparse(), Format::LlamaParse);
    if let Err(e) = skeleton_diff(&fixture(Format::LlamaParse), &rendered, Format::LlamaParse, true) {
        panic!("llamaparse self round-trip: {e}");
    }
}

/// The renders must survive being fed back to the provider's own parser: a caller that swaps
/// providers behind an existing integration is doing exactly this.
#[test]
fn renders_deserialize_with_the_provider_wire_types() {
    use crate::providers::{extend, llamaparse, reducto};

    let v = render_parse(&unified_extend(), Format::Reducto);
    let re: reducto::WireParseResponse = serde_json::from_value(v).expect("reducto wire types accept our render");
    assert!(matches!(re.result, reducto::ParseResult::Full(_)));

    let v = render_parse(&unified_reducto(), Format::Extend);
    let run: extend::ParseRun = serde_json::from_value(v).expect("extend wire types accept our render");
    assert_eq!(run.status, "PROCESSED");
    assert!(run.output.is_some());

    let v = render_parse(&unified_reducto(), Format::LlamaParse);
    let lp: llamaparse::JsonResult = serde_json::from_value(v).expect("llamaparse wire types accept our render");
    assert_eq!(lp.pages.len(), 1);
}

/// Renders are stable: normalise → render → normalise → render must be a fixed point.
#[test]
fn reducto_render_is_a_fixed_point() {
    use crate::providers::reducto::{self, ParseResult};
    let first = render_parse(&unified_reducto(), Format::Reducto);
    let parsed: reducto::WireParseResponse = serde_json::from_value(first.clone()).unwrap();
    let ParseResult::Full(full) = &parsed.result else { panic!() };
    let again = reducto::normalize(&parsed, full, OutputFormat::Markdown, "standard");
    let second = render_parse(&again, Format::Reducto);
    assert_eq!(first["result"], second["result"], "re-rendering must not drift");
}

// ---- cross-format --------------------------------------------------------------------------------

/// Every fixture, rendered as every other vendor's shape, must satisfy that vendor's skeleton:
/// its top-level key set, one unit per unified page, one block per unified block, and the same
/// content strings. Block types are not compared — vendors do not share a vocabulary.
#[test]
fn every_fixture_renders_into_every_native_format() {
    for &src in NATIVE {
        let resp = unified(src);
        for &target in NATIVE {
            let rendered = render_parse(&resp, target);
            // 1. The shape must match the target vendor's own payload skeleton.
            let target_fixture = fixture(target);
            let (e, a) = (skeleton(&target_fixture, target), skeleton(&rendered, target));
            assert_eq!(
                e.top_keys,
                a.top_keys,
                "{src} -> {target}: top-level keys differ (missing {:?}, unexpected {:?})",
                e.top_keys.difference(&a.top_keys).collect::<Vec<_>>(),
                a.top_keys.difference(&e.top_keys).collect::<Vec<_>>(),
            );
            // 2. Nothing may be dropped on the way through.
            assert_eq!(a.units.len(), resp.pages.len(), "{src} -> {target}: page/chunk count");
            assert_eq!(a.usage_pages, Some(resp.usage.pages as u64), "{src} -> {target}: billed pages");
            for (i, page) in resp.pages.iter().enumerate() {
                assert_eq!(a.units[i].content, page.markdown.trim(), "{src} -> {target}: page {i} content");
                assert_eq!(a.units[i].blocks.len(), page.blocks.len(), "{src} -> {target}: page {i} block count");
                for (j, b) in page.blocks.iter().enumerate() {
                    assert_eq!(a.units[i].blocks[j].content, b.content.trim(), "{src} -> {target}: block {i}/{j}");
                    if let (Some(unified_bb), Some(got)) = (b.bbox, a.units[i].blocks[j].bbox) {
                        let want = [unified_bb.x0, unified_bb.y0, unified_bb.x1, unified_bb.y1];
                        assert!(
                            want.iter().zip(&got).all(|(p, q)| (p - q).abs() <= BBOX_EPS),
                            "{src} -> {target}: block {i}/{j} bbox {got:?} != {want:?}"
                        );
                    }
                }
            }
        }
    }
}

/// A spot check with named expectations, so a regression reads as a story rather than a loop index.
#[test]
fn extend_fixture_rendered_as_reducto_is_reducto_shaped() {
    let v = render_parse(&unified_extend(), Format::Reducto);
    assert_eq!(v["response_type"], "parse");
    assert_eq!(v["result"]["type"], "full");
    assert_eq!(v["result"]["chunks"].as_array().unwrap().len(), 2, "two pages, two chunks");
    assert_eq!(v["usage"]["num_pages"], 2);
    let first = &v["result"]["chunks"][0]["blocks"][0];
    assert_eq!(first["type"], "Title", "extend `heading` becomes reducto `Title`");
    assert_eq!(first["content"], "# Hello LiteOCR");
    assert_eq!(first["confidence"], "high");
    // Extend's page-pixel box comes back as a normalised reducto box.
    assert!((f(&first["bbox"]["left"]) - 92.84771095942524 / 1241.0).abs() < BBOX_EPS);
    assert_eq!(first["bbox"]["page"], 1);
    // Extend's `key_value` has no reducto equivalent and lands on `Text`.
    assert_eq!(v["result"]["chunks"][0]["blocks"][1]["type"], "Text");
}

#[test]
fn llamaparse_fixture_rendered_as_extend_is_extend_shaped() {
    let v = render_parse(&unified_llamaparse(), Format::Extend);
    assert_eq!(v["object"], "parse_run");
    assert_eq!(v["status"], "PROCESSED");
    assert_eq!(v["output"]["chunks"].as_array().unwrap().len(), 2);
    assert_eq!(v["metrics"]["pageCount"], 2);
    assert_eq!(v["output"]["chunks"][0]["blocks"][0]["type"], "heading");
    // LlamaParse reports real page dimensions, so nothing is synthesised.
    assert_eq!(v["metadata"], Value::Null);
    assert_eq!(v["output"]["chunks"][0]["blocks"][0]["metadata"]["page"]["width"], 1000.0);
    assert_eq!(v["output"]["chunks"][1]["blocks"][1]["type"], "table");
}

#[test]
fn reducto_fixture_rendered_as_llamaparse_flags_missing_page_size() {
    // Reducto never reports page dimensions (its boxes are already normalised), so the LlamaParse
    // render has to invent the 1000x1000 page the unit boxes are expressed in.
    let v = render_parse(&unified_reducto(), Format::LlamaParse);
    assert_eq!(v["pages"][0]["width"], 1000.0);
    assert_eq!(v["pages"][0]["height"], 1000.0);
    assert_eq!(v["pages"][0]["items"][0]["type"], "heading");
    assert_eq!(v["pages"][0]["items"][0]["lvl"], 1);
    assert!((f(&v["pages"][0]["items"][0]["bBox"]["x"]) - 119.281045751634).abs() < 1e-9);
    assert_eq!(v["job_metadata"]["job_pages"], 1);
    // The same response as Extend records the synthesised dimensions in the free-form metadata map.
    let e = render_parse(&unified_reducto(), Format::Extend);
    assert_eq!(e["metadata"]["liteocr_synthetic_page_dims"], true);
}

// ---- degenerate input ----------------------------------------------------------------------------

/// Pages with no dimensions and blocks with no boxes: nothing may panic, and every format must
/// still produce its envelope.
#[test]
fn every_format_renders_a_response_without_geometry() {
    use crate::types::{Block, Page, Usage};

    let blocks: Vec<Block> = BlockTypeAll::all()
        .into_iter()
        .map(|t| Block {
            block_type: t,
            content: format!("{} content", t.as_str()),
            text: None,
            bbox: None,
            confidence: None,
            page_number: 1,
        })
        .collect();
    let pages = vec![
        Page {
            page_number: 1,
            width: None,
            height: None,
            markdown: "# No geometry".into(),
            text: "No geometry".into(),
            blocks,
        },
        // An entirely empty page, which providers do emit for blank scans.
        Page {
            page_number: 2,
            width: None,
            height: None,
            markdown: String::new(),
            text: String::new(),
            blocks: vec![],
        },
    ];
    let resp = ParseResponse::from_pages("x", "x/y", pages, Usage { pages: 2, ..Default::default() });

    for &format in Format::ALL {
        let v = render_parse(&resp, format);
        assert!(v.is_object(), "{format} must render an object");
        match format {
            Format::Liteocr => assert_eq!(v["pages"].as_array().unwrap().len(), 2),
            Format::Reducto => {
                assert_eq!(v["result"]["chunks"].as_array().unwrap().len(), 2);
                let b = &v["result"]["chunks"][0]["blocks"][0];
                assert_eq!(b["bbox"]["width"], 0.0);
                assert_eq!(b["bbox"]["page"], 1);
                assert_eq!(b["confidence"], Value::Null);
                assert_eq!(v["usage"]["credits"], Value::Null);
            }
            Format::Extend => {
                assert_eq!(v["output"]["chunks"].as_array().unwrap().len(), 2);
                assert_eq!(v["output"]["chunks"][0]["blocks"][0]["boundingBox"], Value::Null);
                assert_eq!(v["metadata"]["liteocr_synthetic_page_dims"], true);
                assert_eq!(v["output"]["metadata"]["pages"][1]["number"], 2);
            }
            Format::LlamaParse => {
                assert_eq!(v["pages"].as_array().unwrap().len(), 2);
                assert_eq!(v["pages"][0]["items"][0]["bBox"], Value::Null);
                assert_eq!(v["pages"][1]["items"].as_array().unwrap().len(), 0);
                assert_eq!(v["job_metadata"]["credits_used"], 0.0);
            }
        }
        // And every render must survive a serialise/parse cycle unchanged.
        let text = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), v);
    }
}

/// Helper so the test above stays exhaustive when a new `BlockType` is added.
struct BlockTypeAll;

impl BlockTypeAll {
    fn all() -> Vec<BlockType> {
        vec![
            BlockType::Text,
            BlockType::Title,
            BlockType::SectionHeader,
            BlockType::List,
            BlockType::Table,
            BlockType::Figure,
            BlockType::Header,
            BlockType::Footer,
            BlockType::Footnote,
            BlockType::Caption,
            BlockType::Formula,
            BlockType::Other,
        ]
    }
}

/// Every unified block type must map into a vocabulary the vendor's own reader understands.
#[test]
fn reverse_maps_stay_inside_each_vendor_vocabulary() {
    for t in BlockTypeAll::all() {
        let r = super::reducto::block_type(t);
        assert!(
            ["Title", "Section Header", "Text", "List Item", "Table", "Figure", "Header", "Footer"].contains(&r),
            "{t:?} -> {r}"
        );
        let e = super::extend::block_type(t);
        assert!(
            ["text", "heading", "section_heading", "table", "figure", "formula", "header", "footer"].contains(&e),
            "{t:?} -> {e}"
        );
        let l = super::llamaparse::item_type(t);
        assert!(["text", "heading", "table"].contains(&l), "{t:?} -> {l}");
    }
}

/// The documented lossy edges, pinned so a change to them is deliberate.
#[test]
fn lossy_type_mappings_are_pinned() {
    // Reducto: four unified types collapse onto `Text`.
    for t in [BlockType::Footnote, BlockType::Caption, BlockType::Formula, BlockType::Other] {
        assert_eq!(super::reducto::block_type(t), "Text");
        assert_eq!(reducto_block_type(super::reducto::block_type(t)), BlockType::Text);
    }
    // Extend: lists, footnotes and captions collapse onto `text`.
    for t in [BlockType::List, BlockType::Footnote, BlockType::Caption] {
        assert_eq!(super::extend::block_type(t), "text");
    }
    // LlamaParse keeps only three item types.
    assert_eq!(super::llamaparse::item_type(BlockType::Figure), "text");
    assert_eq!(super::llamaparse::item_type(BlockType::Header), "text");
    // Everything else survives a full type round-trip through each vendor.
    for t in [BlockType::Title, BlockType::SectionHeader, BlockType::Text, BlockType::Table] {
        assert_eq!(reducto_block_type(super::reducto::block_type(t)), t, "reducto {t:?}");
        assert_eq!(extend_block_type(super::extend::block_type(t)), t, "extend {t:?}");
    }
    assert_eq!(super::reducto::block_type(BlockType::List), "List Item");
    assert_eq!(reducto_block_type("List Item"), BlockType::List);
}

/// `skeleton_diff` must actually catch the things it claims to.
#[test]
fn skeleton_diff_detects_each_kind_of_mismatch() {
    let good = render_parse(&unified_reducto(), Format::Reducto);
    let expected = fixture(Format::Reducto);
    assert!(skeleton_diff(&expected, &good, Format::Reducto, true).is_ok());

    let mut missing_key = good.clone();
    missing_key.as_object_mut().unwrap().remove("parse_mode");
    let e = skeleton_diff(&expected, &missing_key, Format::Reducto, true).unwrap_err();
    assert!(e.contains("top-level keys differ") && e.contains("parse_mode"), "{e}");

    let mut wrong_content = good.clone();
    wrong_content["result"]["chunks"][0]["blocks"][0]["content"] = Value::from("nope");
    let e = skeleton_diff(&expected, &wrong_content, Format::Reducto, true).unwrap_err();
    assert!(e.contains("block[0].content differs"), "{e}");

    let mut wrong_type = good.clone();
    wrong_type["result"]["chunks"][0]["blocks"][0]["type"] = Value::from("Table");
    let e = skeleton_diff(&expected, &wrong_type, Format::Reducto, true).unwrap_err();
    assert!(e.contains("type differs"), "{e}");

    let mut wrong_bbox = good.clone();
    wrong_bbox["result"]["chunks"][0]["blocks"][0]["bbox"]["left"] = Value::from(0.5);
    let e = skeleton_diff(&expected, &wrong_bbox, Format::Reducto, true).unwrap_err();
    assert!(e.contains("bbox differs"), "{e}");

    let mut wrong_pages = good.clone();
    wrong_pages["usage"]["num_pages"] = Value::from(9);
    let e = skeleton_diff(&expected, &wrong_pages, Format::Reducto, true).unwrap_err();
    assert!(e.contains("billed page count differs"), "{e}");

    let mut fewer_blocks = good.clone();
    fewer_blocks["result"]["chunks"][0]["blocks"].as_array_mut().unwrap().pop();
    let e = skeleton_diff(&expected, &fewer_blocks, Format::Reducto, true).unwrap_err();
    assert!(e.contains("block count differs"), "{e}");

    let mut no_chunks = good;
    no_chunks["result"]["chunks"] = Value::Array(vec![]);
    let e = skeleton_diff(&expected, &no_chunks, Format::Reducto, true).unwrap_err();
    assert!(e.contains("chunk/page count differs"), "{e}");
}

/// `to_format` is the surface the SDK and CLI call; it must accept the documented aliases and
/// reject anything else before a request is sent.
#[test]
fn to_format_accepts_aliases_and_rejects_junk() {
    let resp = unified_reducto();
    assert_eq!(resp.to_format("reducto").unwrap()["response_type"], "parse");
    assert_eq!(resp.to_format("EXTEND").unwrap()["object"], "parse_run");
    assert!(resp.to_format("llama").unwrap()["pages"].is_array());
    assert_eq!(resp.to_format("liteocr").unwrap()["provider"], "reducto");
    let err = resp.to_format("docling").unwrap_err();
    assert_eq!(err.kind, crate::error::ErrorKind::Input);
}
