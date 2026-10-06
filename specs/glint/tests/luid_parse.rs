//! Traces: GLINT-LUID-PARSE (canonical spec: specs/glint/spec.md)

use glint::luid::{
    engine_family, parse_engine_instance, AdapterLuid, EngineInstance, ParseError,
};

#[test]
fn parses_the_spec_example() {
    let got = parse_engine_instance("luid_0x00000000_0x0000CAFE_phys_0_eng_1_engtype_3D").unwrap();
    assert_eq!(
        got,
        EngineInstance {
            pid: None,
            luid: AdapterLuid {
                high: 0x00000000,
                low: 0x0000CAFE
            },
            phys: 0,
            eng: 1,
            engtype: "3D",
        }
    );
}

#[test]
fn parses_a_real_instance_name_with_a_pid() {
    let got =
        parse_engine_instance("pid_13080_luid_0x00000000_0x00016BCB_phys_0_eng_1_engtype_Copy")
            .unwrap();
    assert_eq!(
        got,
        EngineInstance {
            pid: Some(13080),
            luid: AdapterLuid {
                high: 0x00000000,
                low: 0x00016BCB
            },
            phys: 0,
            eng: 1,
            engtype: "Copy",
        }
    );
}

#[test]
fn keeps_an_engine_type_that_has_a_space_and_an_index() {
    let got = parse_engine_instance(
        "pid_13080_luid_0x00000000_0x00016BCB_phys_0_eng_2_engtype_Compute 0",
    )
    .unwrap();
    assert_eq!(got.engtype, "Compute 0");
    assert_eq!(got.eng, 2);
    assert_eq!(engine_family(got.engtype), "Compute");
}

#[test]
fn parses_a_two_digit_engine_index() {
    let got =
        parse_engine_instance("pid_4_luid_0x00000000_0x0001B3D1_phys_0_eng_11_engtype_3D").unwrap();
    assert_eq!(got.eng, 11);
    assert_eq!(got.pid, Some(4));
    assert_eq!(got.luid.low, 0x0001B3D1);
}

#[test]
fn parses_a_nonzero_high_part() {
    let got =
        parse_engine_instance("luid_0x0000ABCD_0x12345678_phys_1_eng_0_engtype_VideoDecode")
            .unwrap();
    assert_eq!(
        got.luid,
        AdapterLuid {
            high: 0x0000ABCD,
            low: 0x12345678
        }
    );
    assert_eq!(got.phys, 1);
    assert_eq!(got.engtype, "VideoDecode");
}

#[test]
fn strips_the_trailing_parenthesis_of_a_full_counter_path_fragment() {
    let got =
        parse_engine_instance("pid_12612_luid_0x00000000_0x0001B433_phys_0_eng_0_engtype_Compute)")
            .unwrap();
    assert_eq!(got.engtype, "Compute");
}

#[test]
fn rejects_a_name_with_no_luid_part() {
    assert_eq!(
        parse_engine_instance("pid_13080_phys_0_eng_1_engtype_3D"),
        Err(ParseError::NoLuid)
    );
}

#[test]
fn rejects_a_truncated_luid_rather_than_returning_a_wrong_one() {
    assert_eq!(
        parse_engine_instance("luid_0x00000000_phys_0_eng_1_engtype_3D"),
        Err(ParseError::BadHex)
    );
}

#[test]
fn rejects_a_luid_half_that_has_no_hex_digits() {
    assert_eq!(
        parse_engine_instance("luid_0x_0x0000CAFE_phys_0_eng_1_engtype_3D"),
        Err(ParseError::BadHex)
    );
}

#[test]
fn rejects_a_luid_half_that_lacks_the_hex_prefix() {
    assert_eq!(
        parse_engine_instance("luid_00000000_0x0000CAFE_phys_0_eng_1_engtype_3D"),
        Err(ParseError::BadHex)
    );
}

#[test]
fn rejects_a_name_with_no_engine_type() {
    assert_eq!(
        parse_engine_instance("luid_0x00000000_0x0000CAFE_phys_0_eng_1"),
        Err(ParseError::NoEngType)
    );
}

#[test]
fn rejects_an_empty_engine_type() {
    assert_eq!(
        parse_engine_instance("luid_0x00000000_0x0000CAFE_phys_0_eng_1_engtype_"),
        Err(ParseError::NoEngType)
    );
}

#[test]
fn rejects_a_name_with_no_phys_part() {
    assert_eq!(
        parse_engine_instance("luid_0x00000000_0x0000CAFE_eng_1_engtype_3D"),
        Err(ParseError::NoPhys)
    );
}
