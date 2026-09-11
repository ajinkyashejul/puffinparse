//! `liteocr` command-line interface.

mod bench;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use liteocr_core::{OcrRequest, OutputFormat};

#[derive(Parser, Debug)]
#[command(name = "liteocr", version, about = "One API for every OCR provider", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Parse a document (path or URL) with a provider and print the result.
    Parse(ParseArgs),
    /// List providers, models, pricing, and whether an API key is configured.
    Providers {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Run and report the open OCR benchmark.
    #[command(subcommand)]
    Bench(bench::BenchCommand),
}

#[derive(clap::Args, Debug)]
struct ParseArgs {
    /// Local file path or http(s) URL.
    input: String,
    /// Model as "<provider>/<model>" (e.g. reducto/standard, extend/parse_performance, llamaparse/agentic).
    #[arg(short, long, default_value = "reducto")]
    model: String,
    /// Output format.
    #[arg(short, long, value_enum, default_value_t = Format::Markdown)]
    format: Format,
    /// 1-based page selection, e.g. "1-3,7".
    #[arg(short, long)]
    pages: Option<String>,
    /// Language hint (ISO 639-1), forwarded when supported.
    #[arg(short, long)]
    language: Option<String>,
    /// Provider-specific options as a JSON object.
    #[arg(long, value_name = "JSON")]
    options: Option<String>,
    /// Include the provider's raw payload (json format only).
    #[arg(long)]
    raw: bool,
    /// Whole-call timeout in seconds.
    #[arg(long, default_value_t = 300.0)]
    timeout: f64,
    /// Retries on transient errors.
    #[arg(long, default_value_t = 2)]
    max_retries: u32,
    /// Override the API key (otherwise read from the provider's env var).
    #[arg(long, env = "LITEOCR_API_KEY", hide_env_values = true)]
    api_key: Option<String>,
    /// Override the provider base URL.
    #[arg(long)]
    base_url: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Markdown,
    Text,
    Json,
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("LITEOCR_LOG").unwrap_or_else(|_| EnvFilter::new("error"));
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
}

#[tokio::main]
async fn main() {
    init_logging();
    let cli = Cli::parse();
    let code = match run(cli).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e:#}");
            match e.downcast_ref::<liteocr_core::Error>() {
                Some(le)
                    if matches!(
                        le.kind,
                        liteocr_core::ErrorKind::UnsupportedModel | liteocr_core::ErrorKind::Input
                    ) =>
                {
                    2
                }
                _ => 1,
            }
        }
    };
    std::process::exit(code);
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Parse(args) => parse(args).await,
        Command::Providers { json } => providers(json),
        Command::Bench(cmd) => bench::run(cmd).await,
    }
}

async fn parse(args: ParseArgs) -> Result<()> {
    let mut req = OcrRequest::from_str_input(&args.input)
        .model(&args.model)
        .timeout_secs(args.timeout)
        .max_retries(args.max_retries)
        .include_raw(args.raw)
        .output(if args.format == Format::Text { OutputFormat::Text } else { OutputFormat::Markdown });
    if let Some(p) = args.pages {
        req = req.pages(p);
    }
    if let Some(l) = args.language {
        req = req.language(l);
    }
    if let Some(o) = args.options {
        let v: serde_json::Value = serde_json::from_str(&o).context("--options must be a JSON object")?;
        req = req.provider_options(v);
    }
    if let Some(k) = args.api_key {
        req = req.api_key(k);
    }
    if let Some(b) = args.base_url {
        req = req.base_url(b);
    }
    let resp = liteocr_core::ocr(req).await?;
    match args.format {
        Format::Markdown => println!("{}", resp.markdown),
        Format::Text => println!("{}", resp.text),
        Format::Json => println!("{}", serde_json::to_string_pretty(&resp)?),
    }
    if args.format != Format::Json {
        eprintln!(
            "[{}] {} page(s) in {} ms{}",
            resp.model,
            resp.usage.pages,
            resp.latency_ms,
            resp.cost_usd.map(|c| format!(", est. ${c:.4}")).unwrap_or_default()
        );
    }
    Ok(())
}

fn providers(json: bool) -> Result<()> {
    let prices = liteocr_core::pricing::all_prices();
    if json {
        let v: Vec<serde_json::Value> = liteocr_core::PROVIDERS
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "display_name": p.display_name,
                    "env_var": p.env_var,
                    "key_configured": std::env::var(p.env_var).map(|v| !v.is_empty()).unwrap_or(false),
                    "base_url": p.base_url,
                    "docs": p.docs,
                    "models": p.models.iter().map(|m| serde_json::json!({
                        "model": m.qualified(),
                        "default": m.default,
                        "description": m.description,
                        "per_page_usd": prices.get(&m.qualified()).map(|e| e.per_page_usd),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let mut table = comfy_table::Table::new();
    table.load_style(comfy_table::presets::UTF8_FULL_CONDENSED);
    table.set_header(["Model", "Default", "Key", "$/page", "Description"]);
    for p in liteocr_core::PROVIDERS {
        let key = if std::env::var(p.env_var).map(|v| !v.is_empty()).unwrap_or(false) { "✓" } else { "✗" };
        for m in p.models {
            let price =
                prices.get(&m.qualified()).map(|e| format!("{:.5}", e.per_page_usd)).unwrap_or_else(|| "?".into());
            table.add_row([
                m.qualified(),
                if m.default { "*".into() } else { String::new() },
                key.into(),
                price,
                m.description.into(),
            ]);
        }
    }
    println!("{table}");
    println!(
        "\nKeys are read from: {}",
        liteocr_core::PROVIDERS.iter().map(|p| p.env_var).collect::<Vec<_>>().join(", ")
    );
    Ok(())
}
