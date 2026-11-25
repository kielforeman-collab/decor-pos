//! Minimal CSV reader/writer shared by the catalog importer and the
//! CSV ASN/invoice intake.
//!
//! Hand-rolled (no `csv` crate in the offline cache): RFC 4180 basics —
//! comma separators, double-quoted fields, `""` escapes, CRLF or LF
//! line endings. Money is never parsed here; callers parse fields with
//! `rust_decimal::Decimal`.

use crate::error::{EdiError, Result};

/// Parse CSV text into rows of fields.
///
/// The first row is the header. Blank lines are skipped. An unterminated
/// quote is an error.
pub fn parse_csv(text: &str) -> Result<Vec<Vec<String>>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut field_started = false;

    while let Some(c) = chars.next() {
        match c {
            '"' if !in_quotes && !field_started => {
                in_quotes = true;
                field_started = true;
            }
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            }
            '"' => {
                // A quote in the middle of an unquoted field: keep it
                // literally rather than failing the whole file.
                field.push('"');
            }
            ',' if !in_quotes => {
                row.push(std::mem::take(&mut field));
                field_started = false;
            }
            '\r' if !in_quotes => {
                // CRLF: swallow the following LF.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                end_row(&mut rows, &mut row, &mut field, &mut field_started);
            }
            '\n' if !in_quotes => {
                end_row(&mut rows, &mut row, &mut field, &mut field_started);
            }
            other => {
                field.push(other);
                field_started = true;
            }
        }
    }
    if in_quotes {
        return Err(EdiError::Parse("unterminated quoted field".to_string()));
    }
    if field_started || !field.is_empty() || !row.is_empty() {
        row.push(field);
        push_nonblank_row(&mut rows, row);
    }
    Ok(rows)
}

fn end_row(
    rows: &mut Vec<Vec<String>>,
    row: &mut Vec<String>,
    field: &mut String,
    field_started: &mut bool,
) {
    row.push(std::mem::take(field));
    *field_started = false;
    let finished = std::mem::take(row);
    push_nonblank_row(rows, finished);
}

fn push_nonblank_row(rows: &mut Vec<Vec<String>>, row: Vec<String>) {
    if row.len() == 1 && row[0].trim().is_empty() {
        return;
    }
    rows.push(row);
}

/// Render one CSV row, quoting fields that contain a comma, quote or
/// newline.
pub fn render_csv_row(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|f| {
            if f.contains(',') || f.contains('"') || f.contains('\n') || f.contains('\r') {
                format!("\"{}\"", f.replace('"', "\"\""))
            } else {
                (*f).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Find a column index by header name (case-insensitive, trimmed).
/// Returns `None` when the header row does not contain the name.
pub fn header_index(header: &[String], name: &str) -> Option<usize> {
    header
        .iter()
        .position(|h| h.trim().eq_ignore_ascii_case(name))
}

/// Look up a field by header name in a data row. Missing columns and
/// short rows yield `None`.
pub fn field_by_name<'a>(header: &'a [String], row: &'a [String], name: &str) -> Option<&'a str> {
    header_index(header, name).and_then(|i| row.get(i).map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_fields_and_crlf() {
        let rows = parse_csv("a,b,c\r\n\"x,y\",2,\"say \"\"hi\"\"\"\n").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec!["x,y", "2", "say \"hi\""]);
    }

    #[test]
    fn unterminated_quote_is_an_error() {
        assert!(parse_csv("\"abc\n").is_err());
    }

    #[test]
    fn renders_quoting() {
        assert_eq!(render_csv_row(&["a,b", "plain"]), "\"a,b\",plain");
    }
}
