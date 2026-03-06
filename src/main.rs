mod audio;
mod project;
mod ui;
mod utils;
mod app;

use app::App;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};
use anyhow::Result;
use log::info;
use std::sync::atomic::Ordering;
use clap::Parser;

#[derive(Parser)]
#[command(about = "Application for recording audiobooks by narrators")]
struct Args {
    /// Path to project file (iamreader.json)
    #[arg(default_value = "iamreader.json")]
    project_path: PathBuf,

    /// Enable debug logging
    #[arg(long)]
    debug: bool,

    /// Path to FIFO for external commands
    #[arg(long, value_name = "PATH")]
    fifo: Option<PathBuf>,

    /// Run without UI (headless)
    #[arg(long)]
    headless: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let project_path = args.project_path;
    let debug = args.debug;
    let fifo_path = args.fifo;
    let headless = args.headless;

    let level = if debug {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    let logger = utils::StdoutLogger::new(level);
    log::set_boxed_logger(Box::new(logger))
        .map(|()| log::set_max_level(level))
        .map_err(|e| anyhow::anyhow!("Failed to set logger: {}", e))?;

    let running = Arc::new(AtomicBool::new(true));

    let r = running.clone();
    ctrlc::set_handler(move || {
        info!("Received interrupt signal (Ctrl+C), shutting down...");
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    })?;

    info!("Starting iamreader");
    info!("Project path: {:?}", project_path);
    if debug {
        info!("Debug mode enabled");
    }
    if headless {
        info!("Headless mode enabled");
    }
    if let Some(ref fp) = fifo_path {
        info!("FIFO path: {:?}", fp);
    }

    if !headless {
        let mut app = App::new(project_path.clone(), debug, fifo_path.clone(), false, running.clone())?;
        let mut ui = app.ui.take().ok_or_else(|| anyhow::anyhow!("UI not available"))?;
        let ui_state = ui.get_state();
        let channels = app.take_channels_for_loop();

        // Поток app loop: получает копию пути проекта и каналы от UI-потока,
        // создаёт второй экземпляр App (headless) и крутит run() — обрабатывает действия, fifo, таймеры.
        let project_path_clone = project_path.clone();
        let debug_clone = debug;
        let fifo_path_clone = fifo_path.clone();
        let running_clone = running.clone();
        let app_handle = std::thread::spawn(move || {
            let mut app_loop = match App::new(project_path_clone, debug_clone, fifo_path_clone, true, running_clone) {
                Ok(mut app) => {
                    app.inject_channels(channels);
                    if !app.project.files.is_empty() {
                        app.current_index = Some(app.project.files.len() - 1);
                    }
                    app.update_prev_waveform();
                    app.update_current_waveform();
                    app.set_ui_state(ui_state);
                    let _ = app.update_ui_state();
                    app
                },
                Err(e) => {
                    eprintln!("Failed to create app loop: {:?}", e);
                    return;
                }
            };
            let _ = app_loop.run();
        });

        ui.run().map_err(|e| anyhow::anyhow!("UI error: {:?}", e))?;

        info!("Window closed, stopping application");
        running.store(false, Ordering::SeqCst);

        let _ = app_handle.join();
    } else {
        let mut app = App::new(project_path, debug, fifo_path, headless, running)?;
        app.run()?;
    }

    Ok(())
}
