//! Traces: GLINT-CONFIG-PARSE (canonical spec: specs/glint/spec.md)

use glint::config::Config;

#[test]
fn reads_a_file_that_it_wrote() {
    let text = "{\n  \"x\": -1200,\n  \"y\": 48\n}\n";
    assert_eq!(
        Config::parse(text),
        Config {
            position: Some((-1200, 48)),
        }
    );
}

#[test]
fn reads_a_file_with_no_saved_position() {
    assert_eq!(Config::parse("{}\n"), Config { position: None });
}

#[test]
fn retired_keys_from_an_older_file_are_ignored() {
    let text = "{\n  \"pinned\": true,\n  \"diskExpanded\": false,\n  \"x\": -1200,\n  \"y\": 48\n}\n";
    assert_eq!(Config::parse(text).position, Some((-1200, 48)));
}

#[test]
fn a_damaged_file_falls_back_to_the_defaults() {
    assert_eq!(Config::parse("not json at all"), Config::default());
    assert_eq!(Config::parse(""), Config::default());
    assert_eq!(Config::parse("{\"x\":"), Config::default());
}

#[test]
fn a_half_written_position_is_dropped_rather_than_half_applied() {
    assert_eq!(Config::parse("{\"x\": 10}").position, None);
    assert_eq!(Config::parse("{\"y\": 10}").position, None);
}

#[test]
fn an_unknown_key_is_ignored() {
    let config = Config::parse("{\"theme\": \"neon\", \"x\": 1, \"y\": 2}");
    assert_eq!(config.position, Some((1, 2)));
}
