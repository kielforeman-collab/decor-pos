//! Minimal CSV parser for legacy Decor Fusion exports.
//!
//! The workspace carries no `csv` crate, and the migrator must
//! stay dependency-light and offline, so this is a small
//! hand-rolled parser: RFC 4180 quoting (double-quote escapes,
//! commas and newlines inside quoted fields), CRLF or LF line
//! endings, and a trailing newline optional. It reports an
//! unterminated quote with the line it started on instead of
//! silently truncating the file.

use thiserror::Error;

/// CSV parse errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CsvError {
    /// A quoted field was never closed; `line` is the 1-based
    /// line the field started on.
    #[error("unterminated quoted field starting on line {line}")]
    UnterminatedQuote { line: usize },
}

/// Parse CSV text into rows of fields. The first row is the header
/// (returned as-is; the importers map columns by header name).
/// Blank lines are skipped. Every field is returned untrimmed —
/// callers trim, because a significant space inside a quoted
/// field must survive.
pub fn parse_csv(text: &str) -> Result<Vec<Vec<String>>, CsvError> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut field_started_quoted = false;
    let mut line = 1usize;
    let mut quote_start_line = 1usize;
    let mut chars = text.chars().peekable();
    // True when the current row has seen any non-newline content
    // (so a blank line does not become an empty row).
    let mut row_has_content = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            }
            '"' if !in_quotes && field.is_empty() && !field_started_quoted => {
                in_quotes = true;
                field_started_quoted = true;
                quote_start_line = line;
                row_has_content = true;
            }
            '"' => {
                // A quote in the middle of an unquoted field is
                // kept literally (legacy exports are not always
                // strict) rather than failing the whole file.
                field.push('"');
                row_has_content = true;
            }
            ',' if !in_quotes => {
                row.push(std::mem::take(&mut field));
                field_started_quoted = false;
                row_has_content = true;
            }
            '\r' if !in_quotes => {
                // CRLF: swallow the CR; the LF ends the row.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                finish_row(&mut rows, &mut row, &mut field, &mut row_has_content);
                line += 1;
                field_started_quoted = false;
            }
            '\n' if !in_quotes => {
                finish_row(&mut rows, &mut row, &mut field, &mut row_has_content);
                line += 1;
                field_started_quoted = false;
            }
            _ => {
                if ch == '\n' {
                    line += 1;
                }
                field.push(ch);
                row_has_content = true;
            }
        }
    }
    if in_quotes {
        return Err(CsvError::UnterminatedQuote {
            line: quote_start_line,
        });
    }
    // A trailing partial row (file without a final newline).
    if row_has_content || !field.is_empty() || !row.is_empty() {
        finish_row(&mut rows, &mut row, &mut field, &mut row_has_content);
    }
    Ok(rows)
}

fn finish_row(
    rows: &mut Vec<Vec<String>>,
    row: &mut Vec<String>,
    field: &mut String,
    row_has_content: &mut bool,
) {
    if *row_has_content || !row.is_empty() {
        row.push(std::mem::take(field));
        rows.push(std::mem::take(row));
    }
    *row_has_content = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_rows_and_crlf() {
        let rows = parse_csv("a,b,c\r\n1,2,3\r\n4,5,6").unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec!["a", "b", "c"]);
        assert_eq!(rows[2], vec!["4", "5", "6"]);
    }

    #[test]
    fn quoted_commas_quotes_and_newlines() {
        let rows =
            parse_csv("name,note\n\"Doe, Jane\",\"says \"\"hi\"\"\nsecond line\"\n").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][0], "Doe, Jane");
        assert_eq!(rows[1][1], "says \"hi\"\nsecond line");
    }

    #[test]
    fn blank_lines_are_skipped() {
        let rows = parse_csv("a,b\n\n1,2\n\n").unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn unterminated_quote_errors() {
        assert_eq!(
            parse_csv("a,b\n\"oops,1\n"),
            Err(CsvError::UnterminatedQuote { line: 2 })
        );
    }

    #[test]
    fn empty_text_is_no_rows() {
        assert!(parse_csv("").unwrap().is_empty());
    }
}
