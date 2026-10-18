#![cfg(feature = "egui-ui")]

#[path = "../crates/engine/tests/support/mod.rs"]
mod support;
#[path = "egui/main_window.rs"]
mod main_window;
#[path = "egui/keyboard.rs"]
mod keyboard;
#[path = "egui/dialogs.rs"]
mod dialogs;
#[path = "egui/export.rs"]
mod export;

use eframe::egui::{self, Vec2};
use egui_kittest::Harness;
use iamreader::egui_ui::EguiApp;
use support::Fixture;
use egui_kittest::kittest::Queryable;

fn create_harness(fixture: &Fixture, size: [f32; 2], scale: f32) -> Harness<'static, EguiApp> {
    // Keep separate pointer-event steps inside egui's double-click interval.
    let mut harness = Harness::builder().with_size(Vec2::from(size)).with_step_dt(0.02).with_max_steps(32)
        .with_pixels_per_point(scale).with_theme(egui::Theme::Light)
        .with_context_setup(iamreader::egui_ui::configure_context)
        .build_ui_state(|ui, app: &mut EguiApp| app.show(ui), EguiApp::new(fixture.handle.clone()).unwrap());
    harness.run();

    harness
}

fn set_text<State>(harness: &mut Harness<'_, State>, label: &str, value: &str) {
    harness.get_by_label(label).set_text_value_accesskit(value);
    harness.run();
    assert_eq!(harness.get_by_label(label).value().as_deref(), Some(value));
}
