#[path = "../crates/engine/tests/support/mod.rs"]
mod support;

use iamreader::fifo::{parse_fifo_command, resolve_fifo_command, FifoCommand};
use iamreader_engine::engine::EngineCommand;
use support::{chunk, Fixture};

#[test]
fn preserve_fifo_grammar_and_distinct_numbering_conventions() {
    let fixture = Fixture::create();
    for (line, expected) in [
        ("r", FifoCommand::Record { duration_secs: None }),
        ("RECORD", FifoCommand::Record { duration_secs: None }),
        ("record:12", FifoCommand::Record { duration_secs: Some(12) }),
        ("record:invalid", FifoCommand::Record { duration_secs: None }),
        ("goto 2 p", FifoCommand::Goto { index: Some(2), play: true }),
        ("goto -1", FifoCommand::Goto { index: Some(-1), play: false }),
        ("goto", FifoCommand::Goto { index: Some(-1), play: false }),
        ("e", FifoCommand::Ok),
        ("d", FifoCommand::Stop),
        ("a", FifoCommand::Prev),
        ("f", FifoCommand::Next),
        ("prev_sect", FifoCommand::PrevSect),
        ("next_sect", FifoCommand::NextSect),
        ("s", FifoCommand::Play),
        ("u", FifoCommand::ModeUpdate),
        ("i", FifoCommand::ModeInsert),
        ("m_set 2", FifoCommand::SetMarkers { file_index: 2, markers: vec![] }),
        ("compile", FifoCommand::Compile),
        ("compile_cancel", FifoCommand::CompileCancel),
        ("quit", FifoCommand::Shutdown),
    ] {
        assert_eq!(parse_fifo_command(line).unwrap(), expected);
    }
    assert!(parse_fifo_command("m_add wrong chapter").unwrap_err().to_string().contains("Invalid file index"));
    assert!(parse_fifo_command("unknown").is_err());
    for line in ["trans -2147483648", "trans 0", "trans 4", "m_set 0", "m_set 4", "goto 3"] {
        assert!(resolve_fifo_command(parse_fifo_command(line).unwrap(), &fixture.handle).is_err(), "{line}");
    }
    for (line, expected) in [
        ("m_add 1 chapter", EngineCommand::AddMarkers { chunk: chunk(1), markers: vec!["chapter".into()] }),
        ("m_del 1 chapter", EngineCommand::RemoveMarkers { chunk: chunk(1), markers: vec!["chapter".into()] }),
        ("m_set 2", EngineCommand::SetMarkers { chunk: chunk(2), markers: vec![] }),
        ("trans 1", EngineCommand::Transcribe { chunk: chunk(3) }),
        ("goto 0 play", EngineCommand::Select { chunk: Some(chunk(3)), play: true }),
        ("goto -1", EngineCommand::Select { chunk: None, play: false }),
    ] {
        assert_eq!(resolve_fifo_command(parse_fifo_command(line).unwrap(), &fixture.handle).unwrap(), expected, "{line}");
    }
}
