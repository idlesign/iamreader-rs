//! Immediate-mode desktop client; project and audio operations belong to the engine.

mod app;
pub mod dialogs;
mod file_list;
mod keyboard;
mod notifications;
mod transport;
mod widgets;

pub use app::EguiApp;

/// Configure the same portable fonts and appearance in native windows and UI tests.
pub fn configure_context(context: &eframe::egui::Context) {
    use eframe::egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals, vec2};

    let mut fonts = FontDefinitions::default();
    // Keep the Slint typefaces; default fonts remain fallbacks for symbols.
    for (name, bytes) in [("Ubuntu Regular", include_bytes!("fonts/Ubuntu-Regular.ttf").as_slice()),
        ("Ubuntu Bold", include_bytes!("fonts/Ubuntu-Bold.ttf").as_slice()),
        ("Ubuntu Mono", include_bytes!("fonts/UbuntuMono.ttf").as_slice())] {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    }
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "Ubuntu Regular".into());
    fonts.families.entry(FontFamily::Proportional).or_default().push("Hack".into());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "Ubuntu Mono".into());
    fonts.families.insert(FontFamily::Name("Ubuntu Bold".into()), vec!["Ubuntu Bold".into(), "Ubuntu Regular".into(), "Hack".into()]);
    context.set_fonts(fonts);
    let mut style = (*context.style_of(eframe::egui::Theme::Light)).clone();
    style.visuals = Visuals::light();
    style.visuals.override_text_color = Some(Color32::from_gray(51));
    style.visuals.panel_fill = Color32::from_gray(250);
    style.visuals.window_fill = Color32::from_gray(250);
    style.visuals.extreme_bg_color = Color32::from_gray(240);
    style.visuals.faint_bg_color = Color32::from_gray(245);
    style.visuals.selection.bg_fill = Color32::from_rgb(208, 232, 255);
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(85, 137, 185));
    for visuals in [&mut style.visuals.widgets.inactive, &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active, &mut style.visuals.widgets.open] {
        visuals.corner_radius = CornerRadius::same(3);
        visuals.bg_stroke = Stroke::new(1.0, Color32::from_gray(204));
        visuals.fg_stroke = Stroke::new(1.0, Color32::from_gray(51));
        visuals.expansion = 0.0;
    }
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_gray(250);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_gray(232);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_gray(220);
    style.text_styles.insert(TextStyle::Body, FontId::proportional(16.0));
    style.text_styles.insert(TextStyle::Button, FontId::proportional(16.0));
    style.text_styles.insert(TextStyle::Small, FontId::proportional(14.0));
    style.text_styles.insert(TextStyle::Monospace, FontId::monospace(16.0));
    style.spacing.item_spacing = vec2(6.0, 8.0);
    style.spacing.button_padding = vec2(9.0, 4.0);
    style.spacing.interact_size = vec2(28.0, 32.0);
    context.set_style_of(eframe::egui::Theme::Light, style);
    context.set_theme(eframe::egui::ThemePreference::Light);
}

/// Run the desktop frontend without changing engine ownership.
pub fn run(handle: iamreader_engine::engine::EngineHandle) -> anyhow::Result<()> {
    use eframe::egui;

    let mut app = EguiApp::new(handle)?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([400.0, 300.0])
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("icon.png"))?),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        &format!("iamreader {} {}", env!("CARGO_PKG_VERSION"), env!("BUILD_DATE")),
        options,
        Box::new(move |creation| {
            configure_context(&creation.egui_ctx);
            app.attach_repaint(&creation.egui_ctx)?;
            app.start_monitoring();
            Ok(Box::new(app))
        }),
    ).map_err(|error| anyhow::anyhow!("Egui frontend failed: {error}"))
}
