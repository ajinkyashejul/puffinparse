//! Helpers shared by the self-hosted providers (Tesseract, Docling, PaddleOCR): fetching a
//! document into memory, base64 for JSON bodies, file-type sniffing and a scratch directory that
//! cleans up after itself. None of these providers need an API key.

use crate::error::{Error, Result};
use crate::http::{self, Deadline};
use crate::provider;
use crate::types::{DocumentInput, DocumentRequest};
use std::path::{Path, PathBuf};

/// The document bytes: read from disk / taken from memory, or downloaded when the input is a URL
/// (local engines cannot fetch it themselves, and a self-hosted server may not have egress).
pub(crate) async fn load_or_download(
    provider_name: &str,
    request: &DocumentRequest,
    deadline: &Deadline,
) -> Result<bytes::Bytes> {
    if let Some(data) = provider::load_bytes(&request.input).await? {
        return Ok(data);
    }
    let DocumentInput::Url { url } = &request.input else { unreachable!("load_bytes only returns None for URLs") };
    tracing::debug!(provider = provider_name, %url, "downloading remote document for a local engine");
    let resp = http::client().get(url).timeout(deadline.request_timeout()).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::input(format!("could not download {url}: HTTP {}", status.as_u16())));
    }
    let data = resp.bytes().await?;
    if data.is_empty() {
        return Err(Error::input(format!("{url} returned an empty body")));
    }
    Ok(data)
}

/// What kind of file the bytes are, from magic numbers first and the filename second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileKind {
    Pdf,
    Image,
    Other,
}

pub(crate) fn sniff(data: &[u8], filename: &str) -> FileKind {
    const IMAGE_MAGIC: &[&[u8]] = &[
        b"\x89PNG",
        b"\xFF\xD8\xFF",
        b"GIF8",
        b"II*\x00",
        b"MM\x00*",
        b"BM",
        b"RIFF", // WebP (RIFF....WEBP)
        b"P1",
        b"P2",
        b"P3",
        b"P4",
        b"P5",
        b"P6",
    ];
    if data.starts_with(b"%PDF") {
        return FileKind::Pdf;
    }
    if IMAGE_MAGIC.iter().any(|m| data.starts_with(m)) {
        return FileKind::Image;
    }
    let ext = Path::new(filename).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => FileKind::Pdf,
        "png" | "jpg" | "jpeg" | "gif" | "tif" | "tiff" | "bmp" | "webp" | "pnm" | "pbm" | "pgm" | "ppm" | "jp2" => {
            FileKind::Image
        }
        _ => FileKind::Other,
    }
}

/// Standard base64 (with padding) for JSON request bodies.
pub(crate) fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Keep only the pages selected by a 1-based `"1-3,7"` spec (`None` keeps everything).
pub(crate) fn page_selected(ranges: Option<&[(u32, Option<u32>)]>, page: u32) -> bool {
    match ranges {
        None => true,
        Some(r) => r.iter().any(|&(s, e)| page >= s && e.map(|e| page <= e).unwrap_or(true)),
    }
}

/// A private scratch directory under the system temp dir, removed on drop.
#[derive(Debug)]
pub(crate) struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    pub(crate) async fn new(prefix: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!("liteocr-{prefix}-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| Error::input(format!("cannot create scratch directory {}: {e}", path.display())))?;
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_by_magic_then_extension() {
        assert_eq!(sniff(b"%PDF-1.7", "x.bin"), FileKind::Pdf);
        assert_eq!(sniff(b"\x89PNG\r\n", "scan"), FileKind::Image);
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0", "a.pdf"), FileKind::Image);
        assert_eq!(sniff(b"????", "scan.TIFF"), FileKind::Image);
        assert_eq!(sniff(b"PK\x03\x04", "a.docx"), FileKind::Other);
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn page_selection() {
        let r = [(1, Some(2)), (5, None)];
        assert!(page_selected(Some(&r), 2));
        assert!(!page_selected(Some(&r), 3));
        assert!(page_selected(Some(&r), 9));
        assert!(page_selected(None, 3));
    }

    #[tokio::test]
    async fn scratch_dir_is_removed_on_drop() {
        let dir = ScratchDir::new("test").await.unwrap();
        let p = dir.path().to_path_buf();
        tokio::fs::write(p.join("a.txt"), b"x").await.unwrap();
        drop(dir);
        assert!(!p.exists());
    }
}
