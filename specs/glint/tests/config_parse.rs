//! Traces: GLINT-CONFIG-PARSE (canonical spec: specs/glint/spec.md)

use glint::config::{Config, Show};

#[test]
fn reads_a_file_that_it_wrote() {
    let text = "{\n  \"x\": -1200,\n  \"y\": 48\n}\n";
    assert_eq!(
        Config::parse(text),
        Config {
            position: Some((-1200, 48)),
            ..Config::default()
        }
    );
}

#[test]
fn reads_a_file_with_no_saved_position() {
    assert_eq!(
        Config::parse("{}\n"),
        Config {
            position: None,
            ..Config::default()
        }
    );
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

// --- visibility toggles (GLINT-KPI-TOGGLE persistence) ---

#[test]
fn every_measurement_defaults_to_visible() {
    let show = Config::parse("{}").show;
    assert_eq!(
        show,
        Show {
            cpu: true,
            memory: true,
            gpu: true,
            npu: true,
            disk_activity: true,
            disk_space: true,
        }
    );
}

#[test]
fn a_hidden_measurement_reads_back_hidden() {
    let show = Config::parse("{\"gpu\": false}").show;
    assert!(!show.gpu);
    // The others keep their default, rather than following the one that moved.
    assert!(show.cpu && show.memory && show.npu && show.disk_activity && show.disk_space);
}

#[test]
fn every_measurement_can_be_hidden_at_once() {
    let text = "{\"cpu\": false, \"memory\": false, \"gpu\": false, \
                \"npu\": false, \"diskActivity\": false, \"diskSpace\": false}";
    let show = Config::parse(text).show;
    assert_eq!(
        show,
        Show {
            cpu: false,
            memory: false,
            gpu: false,
            npu: false,
            disk_activity: false,
            disk_space: false,
        }
    );
}

#[test]
fn a_damaged_boolean_falls_back_to_visible() {
    assert!(Config::parse("{\"gpu\": }").show.gpu);
    assert!(Config::parse("{\"gpu\": yes}").show.gpu);
    assert!(Config::parse("{\"gpu\":").show.gpu);
}

#[test]
fn the_two_similar_keys_do_not_cross_match() {
    let show = Config::parse("{\"diskActivity\": false, \"diskSpace\": true}").show;
    assert!(!show.disk_activity);
    assert!(show.disk_space);
}

#[test]
fn a_position_and_a_hidden_row_survive_together() {
    let config = Config::parse("{\"x\": -1200, \"y\": 48, \"npu\": false}");
    assert_eq!(config.position, Some((-1200, 48)));
    assert!(!config.show.npu);
}
