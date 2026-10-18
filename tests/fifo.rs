#[path = "../crates/engine/tests/support/mod.rs"]
mod support;

use iamreader::action::{translate_action, Action};
use iamreader::fifo::parse_fifo_command;
use iamreader_engine::engine::EngineCommand;
use support::{chunk, Fixture};

#[test]
fn preserve_fifo_grammar_and_distinct_numbering_conventions() {
    let fixture = Fixture::create();
    for (line, expected) in [
        ("r", Action::Record { duration_secs: None }),
        ("record:12", Action::Record { duration_secs: Some(12) }),
        ("goto 2 p", Action::Goto { index: Some(2), play: true }),
        ("m_set 2", Action::SetMarkers { file_index: 2, markers: vec![] }),
        ("compile_cancel", Action::CompileCancel),
        ("quit", Action::Shutdown),
    ] {
        assert_eq!(parse_fifo_command(line).unwrap(), expected);
    }
    assert!(parse_fifo_command("m_add wrong chapter").unwrap_err().to_string().contains("Invalid file index"));
    assert!(parse_fifo_command("unknown").is_err());
    assert!(translate_action(Action::Transcribe { file_index: i32::MIN }, &fixture.handle).is_err());
    assert_eq!(translate_action(parse_fifo_command("m_add 1 chapter").unwrap(), &fixture.handle).unwrap(),
        Some(EngineCommand::AddMarkers { chunk: chunk(1), markers: vec!["chapter".into()] }));
    assert_eq!(translate_action(parse_fifo_command("trans 1").unwrap(), &fixture.handle).unwrap(),
        Some(EngineCommand::Transcribe { chunk: chunk(3) }));
}
