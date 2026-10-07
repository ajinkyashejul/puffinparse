# puffinparse-core

Rust core of [PuffinParse](https://github.com/ajinkyashejul/puffinparse): a single API
for every OCR / document-parsing provider (Reducto, Extend, LlamaParse, …).

```rust
use puffinparse_core::{ocr, OcrRequest};

#[tokio::main]
async fn main() -> Result<(), puffinparse_core::Error> {
    let resp = ocr(OcrRequest::from_path("invoice.pdf").model("reducto/standard")).await?;
    println!("{} pages, ${:.4}: {}", resp.usage.pages, resp.cost_usd.unwrap_or(0.0), resp.markdown);
    Ok(())
}
```

See the repository README for the Python SDK, CLI and benchmark.
