use anyhow::Result;
use clap::Parser;
use iamreader::{cli, egui_ui};

#[derive(Parser)]
#[command(about = "Application for recording audiobooks by narrators")]
struct Args {
    #[command(flatten)]
    engine: cli::EngineOptions,
    /// Run without UI (headless)
    #[arg(long)]
    headless: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let frontend = if args.headless { None } else { Some(egui_ui::run as fn(_) -> _) };

    cli::run(args.engine, frontend)
}
