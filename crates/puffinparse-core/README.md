# puffinparse-core

Rust core of [PuffinParse](https://github.com/ajinkyashejul/puffinparse): one API for every
OCR / document-parsing provider (Reducto, Extend, LlamaParse, Mistral, Azure, Textract, …).

```rust
use puffinparse_core::{parse, DocumentRequest};

#[tokio::main]
async fn main() -> Result<(), puffinparse_core::Error> {
    let doc = parse(DocumentRequest::from_path("invoice.pdf").model("reducto/standard")).await?;
    println!("{} pages, ${:.4}: {}", doc.usage.pages, doc.cost_usd.unwrap_or(0.0), doc.markdown);
    Ok(())
}
```

`ocr` returns plain text with line and word boxes, and `extract` fills a JSON Schema; see the
repository README for the Python and TypeScript SDKs, the CLI, the gateway and the benchmark.
