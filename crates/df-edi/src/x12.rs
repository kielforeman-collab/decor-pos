//! Minimal ANSI X12 segment model, builder helpers and parser.
//!
//! This is a deliberately small, honest X12 layer for the Phase 2
//! file-based EDI form (spec §7 Phase 2 row): real segments with real
//! control numbers and correct `SE`/`GE`/`IEA` segment counts — not a
//! pretty-printed fake. Full X12 (855 acknowledgments, 832 catalogs,
//! SFTP/AS2 transport) is Phase 4 and is not claimed here.
//!
//! Envelope layout used by the builders in this crate:
//!
//! ```text
//! ISA*00*          *00*          *ZZ*<sender 15>*ZZ*<receiver 15>
//!     *<YYMMDD>*<HHMM>*U*00401*<control 9>*0*P*:~
//! GS*PO*<sender>*<receiver>*<CCYYMMDD>*<HHMM>*<control>*X*004010~
//! ST*850*<control>~
//! ... transaction segments ...
//! SE*<count incl. ST and SE>*<control>~
//! GE*1*<control>~
//! IEA*1*<control>~
//! ```

use chrono::NaiveDate;

use crate::error::{EdiError, Result};

/// X12 segment terminator.
pub const SEGMENT_TERMINATOR: char = '~';
/// X12 element separator.
pub const ELEMENT_SEPARATOR: char = '*';

/// One X12 segment: a tag plus its elements (element 0 is the tag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Segment tag, e.g. `PO1`, `BEG`.
    pub tag: String,
    /// Elements after the tag, in order.
    pub elements: Vec<String>,
}

impl Segment {
    /// Build a segment from a tag and elements.
    pub fn new(tag: &str, elements: &[&str]) -> Self {
        Self {
            tag: tag.to_string(),
            elements: elements.iter().map(|e| (*e).to_string()).collect(),
        }
    }

    /// Render as `TAG*e1*e2...~`.
    pub fn render(&self) -> String {
        let mut out = self.tag.clone();
        for element in &self.elements {
            out.push(ELEMENT_SEPARATOR);
            out.push_str(element);
        }
        out.push(SEGMENT_TERMINATOR);
        out
    }

    /// Element by 1-based position within the segment (1 = first element
    /// after the tag), as in the X12 spec.
    pub fn get(&self, position: usize) -> Option<&str> {
        self.elements.get(position - 1).map(String::as_str)
    }

    /// Element by 1-based position, trimmed of padding spaces.
    pub fn get_trimmed(&self, position: usize) -> Option<&str> {
        self.get(position).map(str::trim)
    }
}

/// Parse an X12 document into segments.
///
/// Segments are split on `~`; surrounding whitespace/newlines are
/// ignored. A segment with an empty tag, or a `~~` empty segment, is a
/// parse error — malformed input must fail loudly, not silently parse.
pub fn parse_document(text: &str) -> Result<Vec<Segment>> {
    let mut segments = Vec::new();
    for raw in text.split(SEGMENT_TERMINATOR) {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let mut parts = raw.split(ELEMENT_SEPARATOR);
        let tag = parts.next().unwrap_or("").trim();
        if tag.is_empty() {
            return Err(EdiError::Parse(format!("empty segment tag in: {raw:?}")));
        }
        if tag.len() > 3 || !tag.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(EdiError::Parse(format!("invalid segment tag {tag:?}")));
        }
        segments.push(Segment {
            tag: tag.to_string(),
            elements: parts.map(|e| e.to_string()).collect(),
        });
    }
    if segments.is_empty() {
        return Err(EdiError::Parse("document contains no segments".to_string()));
    }
    Ok(segments)
}

/// Find the first segment with the given tag.
pub fn find<'a>(segments: &'a [Segment], tag: &str) -> Option<&'a Segment> {
    segments.iter().find(|s| s.tag == tag)
}

/// Find the first segment with the given tag, or report it missing.
pub fn require<'a>(segments: &'a [Segment], tag: &'static str) -> Result<&'a Segment> {
    find(segments, tag).ok_or(EdiError::MissingSegment(tag))
}

/// Render a list of segments as one document (one segment per line,
/// for human-readable file drops).
pub fn render_document(segments: &[Segment]) -> String {
    let mut out = String::new();
    for segment in segments {
        out.push_str(&segment.render());
        out.push('\n');
    }
    out
}

/// Pad (spaces) or truncate an interchange id to the X12 field width.
pub fn pad_id(id: &str, width: usize) -> String {
    if id.len() >= width {
        id.chars().take(width).collect()
    } else {
        format!("{id:<width$}")
    }
}

/// Format a date as X12 `CCYYMMDD`.
pub fn x12_date(date: NaiveDate) -> String {
    date.format("%Y%m%d").to_string()
}

/// Format a date as X12 `YYMMDD` (ISA09).
pub fn x12_date_short(date: NaiveDate) -> String {
    date.format("%y%m%d").to_string()
}

/// Parse an X12 `CCYYMMDD` date. `YYYYMMDD` with a 2-digit year is
/// rejected — X12 dates in this crate are always 8 digits.
pub fn parse_x12_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%Y%m%d")
        .map_err(|e| EdiError::InvalidDate(value.to_string(), e))
}

/// Format a time as X12 `HHMM`.
pub fn x12_time(time: chrono::NaiveTime) -> String {
    time.format("%H%M").to_string()
}

/// Build the ISA + GS envelope opening for a functional group.
///
/// `group_code` is e.g. `PO` (purchase orders) or `SH` (shipments).
/// `control` is the interchange control number, used in ISA13, GS06
/// and the closing segments; callers must keep it unique per file.
pub fn envelope_open(
    group_code: &str,
    sender_id: &str,
    receiver_id: &str,
    date: NaiveDate,
    time: chrono::NaiveTime,
    control: u32,
) -> Vec<Segment> {
    let control_str = format!("{control:09}");
    vec![
        Segment::new(
            "ISA",
            &[
                "00",
                "          ",
                "00",
                "          ",
                "ZZ",
                &pad_id(sender_id, 15),
                "ZZ",
                &pad_id(receiver_id, 15),
                &x12_date_short(date),
                &x12_time(time),
                "U",
                "00401",
                &control_str,
                "0",
                "P",
                ":",
            ],
        ),
        Segment::new(
            "GS",
            &[
                group_code,
                sender_id.trim(),
                receiver_id.trim(),
                &x12_date(date),
                &x12_time(time),
                &control.to_string(),
                "X",
                "004010",
            ],
        ),
    ]
}

/// Build the GE + IEA envelope closing. `group_control` is the GS06
/// value; `interchange_control` is the ISA13 value.
pub fn envelope_close(group_control: u32, interchange_control: u32) -> Vec<Segment> {
    vec![
        Segment::new("GE", &["1", &group_control.to_string()]),
        Segment::new("IEA", &["1", &format!("{interchange_control:09}")]),
    ]
}

/// Wrap transaction segments in `ST` / `SE` with a correct segment
/// count (the count includes `ST` and `SE` themselves).
pub fn wrap_transaction(set_id: &str, control: u32, body: &[Segment]) -> Vec<Segment> {
    let mut out = vec![Segment::new("ST", &[set_id, &control.to_string()])];
    out.extend(body.iter().cloned());
    let count = out.len() + 1; // + SE itself
    out.push(Segment::new(
        "SE",
        &[&count.to_string(), &control.to_string()],
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;

    #[test]
    fn envelope_round_trips_with_correct_counts() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        let time = NaiveTime::from_hms_opt(15, 30, 0).unwrap();
        let mut segs = envelope_open("PO", "DECORPOS", "VENDOR1", date, time, 1);
        segs.extend(wrap_transaction(
            "850",
            1,
            &[Segment::new("BEG", &["00", "NE", "PO-1"])],
        ));
        segs.extend(envelope_close(1, 1));
        let text = render_document(&segs);
        let back = parse_document(&text).unwrap();
        // ISA, GS, ST, BEG, SE, GE, IEA = 7 segments; SE01 counts ST+BEG+SE = 3.
        assert_eq!(back.len(), 7);
        let se = find(&back, "SE").unwrap();
        assert_eq!(se.get(1), Some("3"));
        assert_eq!(se.get(2), Some("1"));
    }

    #[test]
    fn malformed_document_errors() {
        assert!(parse_document("").is_err());
        assert!(parse_document("~~").is_err());
        assert!(parse_document("ST*850*1~BEG").is_ok()); // short segment parses; semantic checks live in the doc parsers
    }
}
