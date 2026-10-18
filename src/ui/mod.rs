pub mod dialogs;
pub mod export_adapter;
mod file_list;
pub mod ui;
pub mod updater;

pub use ui::FileInfo;

/// Run the Slint frontend; the caller retains the engine thread and shutdown policy.
pub fn run(handle: iamreader_engine::engine::EngineHandle) -> anyhow::Result<()> {
    let bindings = handle.read_snapshot(|snapshot| snapshot.settings.keys.clone())?;
    let (actions, actions_rx) = crossbeam_channel::unbounded();
    let mut frontend = ui::UI::new(actions, bindings, None)?;
    frontend.attach_engine(handle.clone());
    let state = frontend.get_state();
    let mut adapter = updater::SlintEngineAdapter::new(handle.clone(), state.clone());
    adapter.poll()?;
    // Desktop opts into its microphone meter; opening a project alone never does.
    let monitoring = handle.submit(iamreader_engine::engine::EngineCommand::StartMonitoring)?;
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(20), move || {
        for action in actions_rx.try_iter().take(64) {
            if let Err(error) = adapter.dispatch(action) {
                if let Ok(mut state) = state.lock() { state.error_message = format!("{error:#}"); }
            }
        }
        if let Err(error) = adapter.poll() {
            if let Ok(mut state) = state.lock() { state.error_message = format!("{error:#}"); }
        }
        if handle.read_snapshot(|snapshot| snapshot.stopped).unwrap_or(false) {
            if let Err(error) = slint::quit_event_loop() { log::error!("Cannot close frontend: {error}"); }
        }
        if let Ok(Some(iamreader_engine::engine::EngineEvent::CommandCompleted { result: Err(error), .. })) = monitoring.try_recv() {
            if let Ok(mut state) = state.lock() { state.error_message = error.message; }
        }
    });
    let result = frontend.run().map_err(|error| anyhow::anyhow!("UI error: {error:?}"));
    timer.stop();

    result
}
