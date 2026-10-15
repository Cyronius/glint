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
            network: true,
            temp: true,
            disk_space: true,
        }
    );
}

#[test]
fn a_hidden_measurement_reads_back_hidden() {
    let show = Config::parse("{\"gpu\": false}").show;
    assert!(!show.gpu);
    // The others keep their default, rather than following the one that moved.
    assert!(
        show.cpu
            && show.memory
            && show.npu
            && show.disk_activity
            && show.network
            && show.disk_space
    );
}

#[test]
fn a_hidden_network_row_reads_back_hidden() {
    let show = Config::parse("{\"network\": false}").show;
    assert!(!show.network);
    assert!(
        show.cpu && show.memory && show.gpu && show.npu && show.disk_activity && show.disk_space
    );
}

#[test]
fn a_damaged_network_flag_falls_back_to_visible() {
    assert!(Config::parse("{\"network\": }").show.network);
    assert!(Config::parse("{\"network\": yes}").show.network);
    assert!(Config::parse("{\"network\":").show.network);
}

#[test]
fn network_does_not_cross_match_the_other_keys() {
    let show = Config::parse("{\"diskActivity\": false, \"diskSpace\": false}").show;
    assert!(show.network);
    let show = Config::parse("{\"network\": false}").show;
    assert!(show.disk_activity && show.disk_space);
}

#[test]
fn a_hidden_temp_row_reads_back_hidden() {
    let show = Config::parse("{\"temp\": false}").show;
    assert!(!show.temp);
    assert!(show.cpu && show.network && show.disk_activity && show.disk_space);
}

#[test]
fn a_damaged_temp_flag_falls_back_to_visible() {
    assert!(Config::parse("{\"temp\": }").show.temp);
    assert!(Config::parse("{\"temp\": yes}").show.temp);
    assert!(Config::parse("{\"temp\":").show.temp);
}

#[test]
fn temp_and_network_do_not_cross_match() {
    let show = Config::parse("{\"network\": false, \"temp\": true}").show;
    assert!(!show.network && show.temp);
    let show = Config::parse("{\"network\": true, \"temp\": false}").show;
    assert!(show.network && !show.temp);
}

#[test]
fn every_measurement_can_be_hidden_at_once() {
    let text = "{\"cpu\": false, \"memory\": false, \"gpu\": false, \
                \"npu\": false, \"diskActivity\": false, \"network\": false, \
                \"temp\": false, \"diskSpace\": false}";
    let show = Config::parse(text).show;
    assert_eq!(
        show,
        Show {
            cpu: false,
            memory: false,
            gpu: false,
            npu: false,
            disk_activity: false,
            network: false,
            temp: false,
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

#[test]
fn the_install_prompt_is_not_declined_by_default() {
    assert!(!Config::parse("{}").install_declined);
    assert!(!Config::default().install_declined);
}

#[test]
fn a_declined_install_prompt_is_read() {
    assert!(Config::parse("{\"installPrompt\": \"declined\"}").install_declined);
}

#[test]
fn a_damaged_install_prompt_counts_as_not_asked() {
    assert!(!Config::parse("{\"installPrompt\": }").install_declined);
    assert!(!Config::parse("{\"installPrompt\": 5}").install_declined);
    assert!(!Config::parse("{\"installPrompt\":").install_declined);
}

#[test]
fn the_install_prompt_does_not_disturb_other_keys() {
    let config = Config::parse("{\"x\": 7, \"y\": 9, \"gpu\": false, \"installPrompt\": \"declined\"}");
    assert_eq!(config.position, Some((7, 9)));
    assert!(!config.show.gpu);
    assert!(config.show.cpu);
    assert!(config.install_declined);
}
