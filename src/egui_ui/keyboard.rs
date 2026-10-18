use eframe::egui::{Event, Key, Modifiers};
use iamreader_engine::engine::EngineCommand;
use iamreader_engine::project::project::KeyBindings;
use super::dialogs::DialogKind;

pub(super) enum FrontendAction {
    Command(EngineCommand),
    Open(DialogKind),
    Export,
}

pub(super) fn read_actions(events: &[Event], modifiers: Modifiers, bindings: &KeyBindings) -> Vec<FrontendAction> {
    if modifiers.ctrl || modifiers.alt || modifiers.command || modifiers.mac_cmd {
        return Vec::new();
    }
    let has_text = events.iter().any(|event| matches!(event, Event::Text(text) if text.chars().count() == 1));
    events.iter().filter_map(|event| {
        let key = match event {
            Event::Text(text) if text.chars().count() == 1 => normalize_key(text),
            Event::Key { key, pressed: true, repeat: false, modifiers, .. }
                if !modifiers.ctrl && !modifiers.alt && !modifiers.command && !modifiers.mac_cmd => {
                    if has_text && !matches!(key, Key::ArrowUp | Key::ArrowDown | Key::Delete | Key::Escape | Key::F1 | Key::F4 | Key::F5 | Key::F8 | Key::F10) {
                        return None;
                    }
                    normalize_key(key.name())
                }
            _ => return None,
        };
        let action = match key.as_str() {
            "f1" => Some(FrontendAction::Open(DialogKind::Shortcuts)),
            "f4" => Some(FrontendAction::Open(DialogKind::Markers)),
            "f5" => Some(FrontendAction::Open(DialogKind::Project)),
            "f8" => Some(FrontendAction::Export),
            "f10" => Some(FrontendAction::Open(DialogKind::Toc)),
            "up" => Some(FrontendAction::Command(EngineCommand::Next)),
            "down" => Some(FrontendAction::Command(EngineCommand::Prev)),
            _ => None,
        };
        if action.is_some() { return action; }
        if key == normalize_key(&bindings.delete) { return Some(FrontendAction::Open(DialogKind::Delete)); }
        let command = if key == normalize_key(&bindings.record) {
            EngineCommand::Record { duration_secs: None }
        } else if key == normalize_key(&bindings.ok) {
            EngineCommand::Ok
        } else if key == normalize_key(&bindings.stop) {
            EngineCommand::Stop
        } else if key == normalize_key(&bindings.prev) {
            EngineCommand::Prev
        } else if key == normalize_key(&bindings.next) {
            EngineCommand::Next
        } else if key == normalize_key(&bindings.play) {
            EngineCommand::Play
        } else if key == normalize_key(&bindings.chapter_prev) {
            EngineCommand::PrevSect
        } else if key == normalize_key(&bindings.chapter_next) {
            EngineCommand::NextSect
        } else if key == normalize_key(&bindings.mode_update) {
            EngineCommand::ModeUpdate
        } else if key == normalize_key(&bindings.mode_insert) {
            EngineCommand::ModeInsert
        } else if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() {
            EngineCommand::AddMarker { marker: key }
        } else {
            return None;
        };
        Some(FrontendAction::Command(command))
    }).collect()
}

fn normalize_key(key: &str) -> String {
    let mut characters = key.chars();
    let Some(character) = characters.next() else { return String::new(); };
    if characters.next().is_some() { return key.to_ascii_lowercase(); }
    let character = match character.to_lowercase().next().unwrap_or(character) {
        'й' => 'q', 'ц' => 'w', 'у' => 'e', 'к' => 'r', 'е' => 't', 'н' => 'y',
        'г' => 'u', 'ш' => 'i', 'щ' => 'o', 'з' => 'p', 'ф' => 'a', 'ы' => 's',
        'в' => 'd', 'а' => 'f', 'п' => 'g', 'р' => 'h', 'о' => 'j', 'л' => 'k',
        'д' => 'l', 'я' => 'z', 'ч' => 'x', 'с' => 'c', 'м' => 'v', 'и' => 'b',
        'т' => 'n', 'ь' | 'ъ' => 'm', 'б' | ',' => '<', 'ю' | '.' => '>',
        other => other,
    };

    character.to_string()
}
