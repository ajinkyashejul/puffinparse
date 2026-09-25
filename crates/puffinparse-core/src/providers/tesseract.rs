//! Tesseract OCR, run locally by shelling out to the `tesseract` binary (no C bindings).
//!
//! Flow: images go straight to `tesseract <image> stdout [-l lang] [--psm n] [--oem n] tsv`;
//! PDFs are first rasterised with poppler's `pdftoppm -r <dpi> -png`, one image per page. The TSV
//! output carries a row per page / block / paragraph / line / word with pixel boxes and word
//! confidences (0–100), which become:
//!
//! - `ocr` (native): lines + words with normalised boxes and confidences;
//! - `parse` (derived): one `text` block per Tesseract paragraph. Tesseract has no layout model,
//!   so there are no headings, tables or figures.
//!
//! Configuration: `TESSERACT_CMD` (binary, default `tesseract`), `PDFTOPPM_CMD` (default
//! `pdftoppm`), and `provider_options` `lang`, `psm`, `oem`, `dpi`, `config` (a map passed as
//! `-c key=value`), `cmd`. No API key: this is a local engine and costs nothing per page.

use crate::error::{Error, Result};
use crate::http::Deadline;
use crate::provider::Provider;
use crate::providers::local::{self, FileKind, ScratchDir};
use crate::types::{
    BBox, Block, BlockType, DocumentInput, DocumentRequest, Line, OutputFormat, Page, ParseResponse, TextPage,
    TextResponse, Usage, Word,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const NAME: &str = "tesseract";
const ENV_CMD: &str = "TESSERACT_CMD";
const ENV_PDFTOPPM: &str = "PDFTOPPM_CMD";
const DEFAULT_DPI: u32 = 300;

#[derive(Debug, Default, Clone, Copy)]
pub struct Tesseract;

#[async_trait]
impl Provider for Tesseract {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn ocr(&self, request: &DocumentRequest, model: &str) -> Result<TextResponse> {
        let run = run(request).await?;
        let pages: Vec<TextPage> = run.pages.iter().map(TsvPage::to_text_page).collect();
        let usage = Usage { pages: pages.len() as u32, credits: None, provider_cost_usd: None };
        let mut resp = TextResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
        resp.metadata = run.metadata();
        resp.raw = run.raw(request.include_raw);
        Ok(resp)
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let run = run(request).await?;
        let pages: Vec<Page> = run.pages.iter().map(|p| p.to_page(request.output)).collect();
        let usage = Usage { pages: pages.len() as u32, credits: None, provider_cost_usd: None };
        let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
        resp.metadata = run.metadata();
        resp.metadata.insert("puffinparse_derived_from".into(), json!("ocr"));
        resp.raw = run.raw(request.include_raw);
        Ok(resp)
    }
}

// ---- configuration ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Config {
    cmd: String,
    pdftoppm: String,
    lang: Option<String>,
    psm: Option<u32>,
    oem: Option<u32>,
    dpi: Option<u32>,
    vars: Vec<(String, String)>,
}

impl Config {
    fn from_request(request: &DocumentRequest) -> Result<Self> {
        let opt_str = |k: &str| request.option(k).and_then(Value::as_str).map(str::to_string);
        let opt_u32 = |k: &str| -> Result<Option<u32>> {
            match request.option(k) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Number(n)) => {
                    n.as_u64().and_then(|v| u32::try_from(v).ok()).map(Some).ok_or_else(|| {
                        Error::input(format!("tesseract: provider_options.{k} must be a positive integer"))
                    })
                }
                Some(Value::String(s)) => s.trim().parse().map(Some).map_err(|_| {
                    Error::input(format!("tesseract: provider_options.{k} must be an integer, got '{s}'"))
                }),
                Some(other) => {
                    Err(Error::input(format!("tesseract: provider_options.{k} must be an integer, got {other}")))
                }
            }
        };
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let vars = match request.option("config") {
            None | Some(Value::Null) => vec![],
            Some(Value::Object(m)) => m
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
                .collect(),
            Some(_) => {
                return Err(Error::input("tesseract: provider_options.config must be an object of -c variables"))
            }
        };
        Ok(Self {
            cmd: opt_str("cmd").or_else(|| env(ENV_CMD)).unwrap_or_else(|| "tesseract".into()),
            pdftoppm: opt_str("pdftoppm_cmd").or_else(|| env(ENV_PDFTOPPM)).unwrap_or_else(|| "pdftoppm".into()),
            lang: opt_str("lang").or_else(|| request.language.as_deref().map(tesseract_lang)),
            psm: opt_u32("psm")?,
            oem: opt_u32("oem")?,
            dpi: opt_u32("dpi")?,
            vars,
        })
    }

    /// Arguments after `tesseract <image> stdout`.
    fn tesseract_args(&self, dpi: Option<u32>) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(l) = &self.lang {
            args.extend(["-l".to_string(), l.clone()]);
        }
        if let Some(p) = self.psm {
            args.extend(["--psm".to_string(), p.to_string()]);
        }
        if let Some(o) = self.oem {
            args.extend(["--oem".to_string(), o.to_string()]);
        }
        if let Some(d) = dpi {
            args.extend(["--dpi".to_string(), d.to_string()]);
        }
        for (k, v) in &self.vars {
            args.extend(["-c".to_string(), format!("{k}={v}")]);
        }
        args.push("tsv".to_string());
        args
    }
}

/// Map a BCP-47 / ISO 639-1 hint to Tesseract's traineddata names. Anything already in
/// Tesseract form (`eng`, `chi_sim`, `eng+deu`) passes through.
fn tesseract_lang(hint: &str) -> String {
    let h = hint.trim().to_ascii_lowercase();
    let primary = h.split(['-', '_']).next().unwrap_or(&h);
    let mapped = match (primary, h.as_str()) {
        ("zh", "zh-tw" | "zh-hk" | "zh-hant") => "chi_tra",
        ("zh", _) => "chi_sim",
        ("en", _) => "eng",
        ("de", _) => "deu",
        ("fr", _) => "fra",
        ("es", _) => "spa",
        ("it", _) => "ita",
        ("pt", _) => "por",
        ("nl", _) => "nld",
        ("ru", _) => "rus",
        ("ja", _) => "jpn",
        ("ko", _) => "kor",
        ("ar", _) => "ara",
        ("hi", _) => "hin",
        ("pl", _) => "pol",
        ("tr", _) => "tur",
        ("sv", _) => "swe",
        ("da", _) => "dan",
        ("fi", _) => "fin",
        ("no" | "nb", _) => "nor",
        ("cs", _) => "ces",
        ("el", _) => "ell",
        ("he", _) => "heb",
        ("uk", _) => "ukr",
        ("vi", _) => "vie",
        ("th", _) => "tha",
        ("id", _) => "ind",
        _ => return hint.trim().to_string(),
    };
    mapped.to_string()
}

// ---- running the binaries -----------------------------------------------------------------------

#[derive(Debug)]
struct Run {
    pages: Vec<TsvPage>,
    tsv: Vec<(u32, String)>,
    config: Config,
    rasterized: bool,
}

impl Run {
    fn metadata(&self) -> std::collections::BTreeMap<String, Value> {
        let mut m = std::collections::BTreeMap::new();
        m.insert("tesseract_lang".into(), json!(self.config.lang.clone().unwrap_or_else(|| "eng".into())));
        if let Some(psm) = self.config.psm {
            m.insert("tesseract_psm".into(), json!(psm));
        }
        if self.rasterized {
            m.insert("tesseract_pdf_dpi".into(), json!(self.config.dpi.unwrap_or(DEFAULT_DPI)));
        }
        m
    }

    fn raw(&self, include: bool) -> Option<Value> {
        include.then(|| {
            json!({
                "engine": "tesseract",
                "format": "tsv",
                "pages": self.tsv.iter().map(|(n, t)| json!({"page_number": n, "tsv": t})).collect::<Vec<_>>(),
            })
        })
    }
}

async fn run(request: &DocumentRequest) -> Result<Run> {
    let config = Config::from_request(request)?;
    let deadline = Deadline::new(request.timeout_secs);
    let ranges = request.pages.as_deref().map(crate::util::parse_page_ranges).transpose()?;
    let filename = request.input.filename();
    let data = local::load_or_download(NAME, request, &deadline).await?;
    let kind = local::sniff(&data, &filename);
    let scratch = ScratchDir::new(NAME).await?;

    // (page number or None for "take Tesseract's page_num", image path, dpi hint)
    let mut images: Vec<(Option<u32>, PathBuf, Option<u32>)> = Vec::new();
    let rasterized = kind == FileKind::Pdf;
    match kind {
        FileKind::Pdf => {
            let pdf = scratch.path().join("input.pdf");
            write(&pdf, &data).await?;
            let dpi = config.dpi.unwrap_or(DEFAULT_DPI);
            let mut args = vec!["-r".to_string(), dpi.to_string(), "-png".to_string()];
            if let Some(r) = &ranges {
                // Rasterise only the span that covers the selection; pages outside it are skipped below.
                let first = r.iter().map(|(s, _)| *s).min().unwrap_or(1);
                args.extend(["-f".to_string(), first.to_string()]);
                if let Some(last) = r.iter().map(|(_, e)| *e).try_fold(0u32, |acc, e| e.map(|e| acc.max(e))) {
                    args.extend(["-l".to_string(), last.to_string()]);
                }
            }
            args.push(pdf.to_string_lossy().into_owned());
            args.push(scratch.path().join("page").to_string_lossy().into_owned());
            exec(
                &config.pdftoppm,
                &args,
                &deadline,
                "pdftoppm",
                "install poppler-utils (apt install poppler-utils / brew install poppler) or set PDFTOPPM_CMD",
            )
            .await?;
            for (n, path) in rendered_pages(scratch.path()).await? {
                if local::page_selected(ranges.as_deref(), n) {
                    images.push((Some(n), path, Some(dpi)));
                }
            }
            if images.is_empty() {
                return Err(Error::input(format!("{filename}: no pages to OCR (page selection {:?})", request.pages)));
            }
        }
        FileKind::Image => {
            let path = match &request.input {
                DocumentInput::Path { path } => path.clone(),
                _ => {
                    let ext = Path::new(&filename).extension().map(|e| e.to_string_lossy().into_owned());
                    let p = scratch.path().join(format!("input.{}", ext.unwrap_or_else(|| "img".into())));
                    write(&p, &data).await?;
                    p
                }
            };
            images.push((None, path, config.dpi));
        }
        FileKind::Other => {
            let msg = format!("tesseract only reads images and PDFs; '{filename}' is neither");
            return Err(Error::input(format!("{msg} (convert it first, or use a parse provider)")));
        }
    }

    let mut pages = Vec::new();
    let mut tsv_out = Vec::new();
    for (page_number, image, dpi) in images {
        let mut args = vec![image.to_string_lossy().into_owned(), "stdout".to_string()];
        args.extend(config.tesseract_args(dpi));
        let stdout = exec(
            &config.cmd,
            &args,
            &deadline,
            "tesseract",
            "install Tesseract (apt install tesseract-ocr / brew install tesseract) or set TESSERACT_CMD",
        )
        .await?;
        let tsv = String::from_utf8_lossy(&stdout).into_owned();
        for mut page in parse_tsv(&tsv)? {
            if let Some(n) = page_number {
                page.page_number = n;
            } else if !local::page_selected(ranges.as_deref(), page.page_number) {
                continue; // multi-page TIFF with a page selection
            }
            pages.push(page);
        }
        tsv_out.push((page_number.unwrap_or(1), tsv));
    }
    Ok(Run { pages, tsv: tsv_out, config, rasterized })
}

async fn write(path: &Path, data: &[u8]) -> Result<()> {
    tokio::fs::write(path, data).await.map_err(|e| Error::input(format!("cannot write {}: {e}", path.display())))
}

/// `pdftoppm` names pages `page-1.png` or `page-01.png` (zero-padded to the page count's width).
async fn rendered_pages(dir: &Path) -> Result<Vec<(u32, PathBuf)>> {
    let mut out = Vec::new();
    let mut rd = tokio::fs::read_dir(dir).await.map_err(|e| Error::provider(format!("pdftoppm output: {e}")))?;
    while let Some(entry) = rd.next_entry().await.map_err(|e| Error::provider(format!("pdftoppm output: {e}")))? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix("page-").and_then(|s| s.strip_suffix(".png")).and_then(|s| s.parse().ok()) {
            out.push((n, entry.path()));
        }
    }
    out.sort();
    Ok(out)
}

/// Run a local binary with the call's deadline; non-zero exits carry the tool's stderr verbatim.
async fn exec(cmd: &str, args: &[String], deadline: &Deadline, tool: &str, install_hint: &str) -> Result<Vec<u8>> {
    let mut command = tokio::process::Command::new(cmd);
    // Tesseract's OpenMP threads oversubscribe the CPU when several pages run concurrently (a
    // small PNG took 70 s instead of 0.7 s on 4 cores); one thread per process is the documented
    // fix. An explicit OMP_THREAD_LIMIT in the environment still wins.
    if std::env::var_os("OMP_THREAD_LIMIT").is_none() {
        command.env("OMP_THREAD_LIMIT", "1");
    }
    let child = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::provider(format!("{tool} binary '{cmd}' not found on PATH: {install_hint}"))
            } else {
                Error::provider(format!("could not start {tool} ('{cmd}'): {e}"))
            }
            .with_provider(NAME)
        })?;
    let output = match tokio::time::timeout(deadline.remaining(), child.wait_with_output()).await {
        Ok(r) => r.map_err(|e| Error::provider(format!("{tool} failed: {e}")).with_provider(NAME))?,
        Err(_) => return Err(Error::timeout(format!("deadline exceeded while running {tool}")).with_provider(NAME)),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(
            Error::provider(format!("{tool} exited with {}: {}", output.status, stderr.trim())).with_provider(NAME)
        );
    }
    Ok(output.stdout)
}

// ---- TSV → unified types -----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Px {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone)]
struct TsvWord {
    text: String,
    px: Px,
    conf: Option<f64>,
}

#[derive(Debug, Clone)]
struct TsvLine {
    px: Px,
    words: Vec<TsvWord>,
}

#[derive(Debug, Clone)]
struct TsvPara {
    px: Px,
    lines: Vec<TsvLine>,
}

#[derive(Debug, Clone)]
struct TsvPage {
    page_number: u32,
    width: f64,
    height: f64,
    paragraphs: Vec<TsvPara>,
}

/// Parse Tesseract's TSV renderer output (`level page_num block_num par_num line_num word_num
/// left top width height conf text`). Rows arrive in document order; words with empty text
/// (layout rows, blank detections) are dropped, as are lines and paragraphs left empty.
fn parse_tsv(tsv: &str) -> Result<Vec<TsvPage>> {
    let mut pages: Vec<TsvPage> = Vec::new();
    for (i, row) in tsv.lines().enumerate() {
        if i == 0 && row.starts_with("level") {
            continue;
        }
        if row.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = row.splitn(12, '\t').collect();
        if cols.len() < 11 {
            return Err(Error::provider(format!("tesseract: malformed TSV row {}: '{row}'", i + 1)).with_provider(NAME));
        }
        let num = |k: usize| -> Result<f64> {
            cols[k].trim().parse::<f64>().map_err(|_| {
                Error::provider(format!("tesseract: malformed TSV row {} column {}: '{}'", i + 1, k + 1, cols[k]))
                    .with_provider(NAME)
            })
        };
        let level = num(0)? as u32;
        let px = Px { left: num(6)?, top: num(7)?, width: num(8)?, height: num(9)? };
        match level {
            1 => pages.push(TsvPage {
                page_number: num(1)? as u32,
                width: px.width,
                height: px.height,
                paragraphs: vec![],
            }),
            3 => {
                if let Some(p) = pages.last_mut() {
                    p.paragraphs.push(TsvPara { px, lines: vec![] });
                }
            }
            4 => {
                if let Some(para) = pages.last_mut().and_then(|p| p.paragraphs.last_mut()) {
                    para.lines.push(TsvLine { px, words: vec![] });
                }
            }
            5 => {
                let text = cols.get(11).map(|t| t.trim()).unwrap_or_default();
                if text.is_empty() {
                    continue;
                }
                let conf = num(10)?;
                let word =
                    TsvWord { text: text.to_string(), px, conf: (conf >= 0.0).then(|| (conf / 100.0).clamp(0.0, 1.0)) };
                if let Some(line) =
                    pages.last_mut().and_then(|p| p.paragraphs.last_mut()).and_then(|para| para.lines.last_mut())
                {
                    line.words.push(word);
                }
            }
            _ => {} // 2 = block: paragraphs carry the structure we need
        }
    }
    for p in &mut pages {
        for para in &mut p.paragraphs {
            para.lines.retain(|l| !l.words.is_empty());
        }
        p.paragraphs.retain(|para| !para.lines.is_empty());
    }
    Ok(pages)
}

fn mean(confs: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let v: Vec<f64> = confs.flatten().collect();
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

impl TsvLine {
    fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

impl TsvPara {
    fn text(&self) -> String {
        self.lines.iter().map(TsvLine::text).collect::<Vec<_>>().join("\n")
    }
}

impl TsvPage {
    fn bbox(&self, px: &Px) -> Option<BBox> {
        BBox::from_xywh(px.left, px.top, px.width, px.height, self.width, self.height)
    }

    fn to_text_page(&self) -> TextPage {
        let lines: Vec<&TsvLine> = self.paragraphs.iter().flat_map(|p| p.lines.iter()).collect();
        TextPage {
            page_number: self.page_number,
            width: Some(self.width),
            height: Some(self.height),
            text: lines.iter().map(|l| l.text()).collect::<Vec<_>>().join("\n"),
            lines: lines
                .iter()
                .map(|l| Line {
                    text: l.text(),
                    bbox: self.bbox(&l.px),
                    confidence: mean(l.words.iter().map(|w| w.conf)),
                })
                .collect(),
            words: lines
                .iter()
                .flat_map(|l| l.words.iter())
                .map(|w| Word { text: w.text.clone(), bbox: self.bbox(&w.px), confidence: w.conf })
                .collect(),
        }
    }

    fn to_page(&self, _fmt: OutputFormat) -> Page {
        // Tesseract text carries no markdown syntax, so markdown and text renderings coincide.
        let blocks: Vec<Block> = self
            .paragraphs
            .iter()
            .map(|para| {
                let text = para.text();
                Block {
                    block_type: BlockType::Text,
                    content: text.clone(),
                    text: Some(text),
                    bbox: self.bbox(&para.px),
                    confidence: mean(para.lines.iter().flat_map(|l| l.words.iter()).map(|w| w.conf)),
                    page_number: self.page_number,
                }
            })
            .collect();
        let dims = std::collections::BTreeMap::from([(self.page_number, (self.width, self.height))]);
        crate::types::pages_from_blocks(blocks, &dims).pop().unwrap_or(Page {
            page_number: self.page_number,
            width: Some(self.width),
            height: Some(self.height),
            markdown: String::new(),
            text: String::new(),
            blocks: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/tesseract_headings.tsv");

    #[test]
    fn parses_real_tsv_into_lines_words_and_paragraphs() {
        let pages = parse_tsv(FIXTURE).unwrap();
        assert_eq!(pages.len(), 1);
        let p = &pages[0];
        assert_eq!((p.width, p.height), (1240.0, 1754.0));
        assert!(p.paragraphs.len() >= 4, "{}", p.paragraphs.len());

        let tp = p.to_text_page();
        assert!(tp.text.starts_with("Notes on Coastal Erosion"), "{}", tp.text);
        assert_eq!(tp.lines[0].text, "Notes on Coastal Erosion");
        let first = tp.words.first().unwrap();
        assert_eq!(first.text, "Notes");
        let bb = first.bbox.unwrap();
        assert!(bb.x0 > 0.0 && bb.x0 < 0.2 && bb.y0 < 0.1 && bb.x1 > bb.x0 && bb.y1 > bb.y0, "{bb:?}");
        let c = first.confidence.unwrap();
        assert!((0.0..=1.0).contains(&c) && c > 0.5, "{c}");
        assert!(tp.lines.iter().all(|l| l.bbox.is_some() && l.confidence.is_some()));
        assert_eq!(tp.words.len(), tp.lines.iter().map(|l| l.text.split(' ').count()).sum::<usize>());

        let page = p.to_page(OutputFormat::Markdown);
        assert_eq!(page.blocks.len(), p.paragraphs.len());
        assert!(page.blocks.iter().all(|b| b.block_type == BlockType::Text && b.bbox.is_some()));
        assert_eq!(page.blocks[0].content, "Notes on Coastal Erosion");
        assert!(page.markdown.contains("\n\n"), "paragraphs are separated by a blank line");
        assert!(page.text.contains("Results and Discussion"));
    }

    #[test]
    fn tsv_edge_cases() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   1\t1\t0\t0\t0\t0\t0\t0\t100\t200\t-1\t\n\
                   2\t1\t1\t0\t0\t0\t10\t10\t50\t20\t-1\t\n\
                   3\t1\t1\t1\t0\t0\t10\t10\t50\t20\t-1\t\n\
                   4\t1\t1\t1\t1\t0\t10\t10\t50\t20\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t10\t10\t20\t20\t95.5\tHi\n\
                   5\t1\t1\t1\t1\t2\t40\t10\t20\t20\t-1\t \n\
                   3\t1\t1\t2\t0\t0\t10\t40\t50\t20\t-1\t\n\
                   4\t1\t1\t2\t1\t0\t10\t40\t50\t20\t-1\t\n\
                   1\t2\t0\t0\t0\t0\t0\t0\t100\t200\t-1\t\n";
        let pages = parse_tsv(tsv).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].paragraphs.len(), 1, "empty paragraph dropped");
        assert_eq!(pages[0].paragraphs[0].lines[0].words.len(), 1, "blank word dropped");
        assert_eq!(pages[0].paragraphs[0].lines[0].words[0].conf, Some(0.955));
        assert_eq!(pages[1].page_number, 2);
        let empty = pages[1].to_page(OutputFormat::Text);
        assert!(empty.blocks.is_empty() && empty.text.is_empty());
        assert!(parse_tsv("5\t1\t1").is_err());
    }

    #[test]
    fn config_from_options_and_language() {
        let req = DocumentRequest::from_path("x.png")
            .language("de")
            .provider_options(json!({"psm": 6, "oem": "1", "dpi": 200, "config": {"preserve_interword_spaces": 1}}));
        let c = Config::from_request(&req).unwrap();
        assert_eq!(c.lang.as_deref(), Some("deu"));
        assert_eq!((c.psm, c.oem, c.dpi), (Some(6), Some(1), Some(200)));
        assert_eq!(
            c.tesseract_args(Some(300)),
            ["-l", "deu", "--psm", "6", "--oem", "1", "--dpi", "300", "-c", "preserve_interword_spaces=1", "tsv"]
        );
        let req = DocumentRequest::from_path("x.png").language("en").provider_options(json!({"lang": "eng+fra"}));
        assert_eq!(Config::from_request(&req).unwrap().lang.as_deref(), Some("eng+fra"));
        assert!(Config::from_request(&DocumentRequest::from_path("x").provider_options(json!({"psm": "x"}))).is_err());
        assert_eq!(tesseract_lang("zh-TW"), "chi_tra");
        assert_eq!(tesseract_lang("chi_sim"), "chi_sim");
    }

    #[tokio::test]
    async fn missing_binary_is_a_clear_error() {
        let req = DocumentRequest::from_bytes(&b"\x89PNG\r\n\x1a\n"[..], "a.png")
            .provider_options(json!({"cmd": "/nonexistent/tesseract-puffinparse"}));
        let e = Tesseract.ocr(&req, "default").await.unwrap_err();
        assert!(e.message.contains("tesseract binary '/nonexistent/tesseract-puffinparse' not found"), "{e}");
        let req = DocumentRequest::from_bytes(&b"%PDF-1.4"[..], "a.pdf")
            .provider_options(json!({"pdftoppm_cmd": "/nonexistent/pdftoppm-puffinparse"}));
        let e = Tesseract.parse(&req, "default").await.unwrap_err();
        assert!(e.message.contains("pdftoppm binary") && e.message.contains("poppler"), "{e}");
    }

    #[tokio::test]
    async fn rejects_non_image_input() {
        let req = DocumentRequest::from_bytes(&b"PK\x03\x04"[..], "a.docx");
        let e = Tesseract.parse(&req, "default").await.unwrap_err();
        assert_eq!(e.kind, crate::error::ErrorKind::Input);
    }

    /// Needs the `tesseract` and `pdftoppm` binaries. Run with:
    /// `cargo test -p puffinparse-core tesseract -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs the tesseract and pdftoppm binaries"]
    async fn live_ocr_image_and_pdf() {
        let docs = concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs");
        let req = DocumentRequest::from_path(format!("{docs}/headings_001.png")).model("tesseract/default");
        let resp = Tesseract.ocr(&req, "default").await.expect("tesseract runs");
        assert!(resp.text.contains("Coastal Erosion"), "{}", resp.text);
        assert!(!resp.pages[0].words.is_empty());

        let req = DocumentRequest::from_path(format!("{docs}/multipage_001.pdf")).model("tesseract/default");
        let resp = Tesseract.parse(&req, "default").await.expect("pdf rasterises");
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.pages.len(), 2);
        assert!(resp.pages.iter().all(|p| !p.blocks.is_empty()));

        let req = DocumentRequest::from_path(format!("{docs}/multipage_001.pdf")).pages("2");
        let resp = Tesseract.ocr(&req, "default").await.expect("page selection");
        assert_eq!(resp.pages.len(), 1);
        assert_eq!(resp.pages[0].page_number, 2);
    }
}
