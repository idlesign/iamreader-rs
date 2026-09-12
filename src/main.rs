mod app;
mod audio;
mod project;
mod ui;
mod utils;

use anyhow::Result;
use app::App;
use clap::Parser;
use log::info;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicBool, Arc};

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
        // Bootstrap only the window here. Devices and background workers belong to the
        // single App created in the command-loop thread, never to the UI thread.
        let project = project::Project::load(&project_path)?;
        let (action_tx, action_rx) = crossbeam_channel::unbounded();
        let (current_index_tx, current_index_rx) = crossbeam_channel::unbounded();
        let mut ui = ui::ui::UI::new(
            action_tx,
            project.settings.keys.clone(),
            Some(current_index_rx),
        )?;
        ui.load_meta_from_project(&project.meta, &project.settings)?;
        drop(project);
        let ui_state = ui.get_state();

        // The command loop owns project mutations, audio devices and transcription.
        let project_path_clone = project_path.clone();
        let debug_clone = debug;
        let fifo_path_clone = fifo_path.clone();
        let running_clone = running.clone();
        let app_handle = std::thread::spawn(move || {
            let mut app_loop = match App::new(
                project_path_clone,
                debug_clone,
                fifo_path_clone,
                true,
                running_clone,
            ) {
                Ok(mut app) => {
                    app.action_rx = action_rx;
                    app.current_index_tx = Some(current_index_tx);
                    if !app.project.files.is_empty() {
                        app.current_index = Some(app.project.files.len() - 1);
                    }
                    app.update_current_waveform();
                    app.update_prev_waveform();
                    app.set_ui_state(ui_state.clone());
                    if let Err(error) = app.update_ui_state() {
                        app.report_error(&error);
                    }
                    app
                }
                Err(e) => {
                    eprintln!("Failed to create app loop: {:?}", e);
                    if let Ok(mut state) = ui_state.lock() {
                        state.error_message = format!(
                            "Application could not start. Correct the error and restart: {e:#}"
                        );
                    }
                    return;
                }
            };
            if let Err(error) = app_loop.run() {
                app_loop.report_error(&error);
            }
        });

        let ui_result = ui.run().map_err(|e| anyhow::anyhow!("UI error: {:?}", e));

        info!("Window closed, stopping application");
        running.store(false, Ordering::SeqCst);

        let _ = app_handle.join();
        ui_result?;
    } else {
        let mut app = App::new(project_path, debug, fifo_path, headless, running)?;
        app.run()?;
    }

    Ok(())
}
