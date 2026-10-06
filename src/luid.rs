//! Parse `GPU Engine` PDH instance names.
//!
//! Traces: GLINT-LUID-PARSE (canonical spec: specs/glint/spec.md)

/// A Windows adapter LUID, as the counter instance name spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AdapterLuid {
    pub high: u32,
    pub low: u32,
}

/// One `GPU Engine` instance: an adapter, a physical node, an engine, a type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineInstance<'a> {
    pub pid: Option<u32>,
    pub luid: AdapterLuid,
    pub phys: u32,
    pub eng: u32,
    pub engtype: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The name has no `luid_0x..._0x...` part.
    NoLuid,
    /// The name has no `phys_<n>` part, or the number does not parse.
    NoPhys,
    /// The name has no `eng_<n>` part, or the number does not parse.
    NoEng,
    /// The name has no `engtype_<name>` part, or the type name is empty.
    NoEngType,
    /// A hexadecimal LUID half does not parse.
    BadHex,
}

/// Read `0x` plus hexadecimal digits at the start of `s`.
/// Returns the value and the rest of `s`.
fn take_hex(s: &str) -> Result<(u32, &str), ParseError> {
    let body = s.strip_prefix("0x").ok_or(ParseError::BadHex)?;
    let end = body
        .find(|c: char| !c.is_ascii_hexdigit())
        .unwrap_or(body.len());
    if end == 0 {
        return Err(ParseError::BadHex);
    }
    let value = u32::from_str_radix(&body[..end], 16).map_err(|_| ParseError::BadHex)?;
    Ok((value, &body[end..]))
}

/// Read decimal digits at the start of `s`.
/// Returns the value and the rest of `s`.
fn take_dec(s: &str) -> Option<(u32, &str)> {
    let end = s
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    Some((s[..end].parse().ok()?, &s[end..]))
}

/// Parse a `GPU Engine` instance name.
///
/// The name looks like
/// `pid_13080_luid_0x00000000_0x00016BCB_phys_0_eng_2_engtype_Compute 0`.
/// The `pid_` part is optional. The engine type runs to the end of the name,
/// so a type such as `Compute 0` keeps its space and its index.
pub fn parse_engine_instance(name: &str) -> Result<EngineInstance<'_>, ParseError> {
    let pid = name
        .strip_prefix("pid_")
        .and_then(take_dec)
        .map(|(value, _)| value);

    // The LUID marker anchors the parse. Everything after it is positional.
    let at_luid = name.find("luid_").ok_or(ParseError::NoLuid)?;
    let rest = &name[at_luid + "luid_".len()..];

    let (high, rest) = take_hex(rest)?;
    let rest = rest.strip_prefix('_').ok_or(ParseError::NoLuid)?;
    let (low, rest) = take_hex(rest)?;

    let rest = match rest.find("phys_") {
        Some(at) => &rest[at + "phys_".len()..],
        None => return Err(ParseError::NoPhys),
    };
    let (phys, rest) = take_dec(rest).ok_or(ParseError::NoPhys)?;

    let rest = match rest.find("eng_") {
        Some(at) => &rest[at + "eng_".len()..],
        None => return Err(ParseError::NoEng),
    };
    let (eng, rest) = take_dec(rest).ok_or(ParseError::NoEng)?;

    let engtype = match rest.find("engtype_") {
        Some(at) => &rest[at + "engtype_".len()..],
        None => return Err(ParseError::NoEngType),
    };
    // PDH wraps the name in parentheses in a full counter path. Drop the tail.
    let engtype = engtype.trim_end_matches(')').trim();
    if engtype.is_empty() {
        return Err(ParseError::NoEngType);
    }

    Ok(EngineInstance {
        pid,
        luid: AdapterLuid { high, low },
        phys,
        eng,
        engtype,
    })
}

/// Group an engine type into the family that the UI sums over.
/// `Compute 0` and `Compute 1` are one family.
pub fn engine_family(engtype: &str) -> &str {
    match engtype.split_whitespace().next() {
        Some(head) if !head.is_empty() => head,
        _ => engtype,
    }
}
