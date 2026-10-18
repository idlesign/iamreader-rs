use anyhow::Result;
use clap::Parser;
use iamreader::cli::{self, EngineOptions};

#[derive(Parser)]
#[command(about = "Audiobook recording engine controlled through FIFO, without a GUI")]
struct Args {
    #[command(flatten)]
    engine: EngineOptions,
}

fn main() -> Result<()> {
    cli::run(Args::parse().engine, None)
}
