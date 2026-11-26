//! Reconciliation manifest: every row of every imported file is
//! counted and checksummed, and the manifest says what happened
//! to it — nothing is silently dropped.
//!
//! * **File checksum** — SHA-256 over the raw file bytes, so a
//!   re-run can prove it read the same file.
//! * **Row checksum** — SHA-256 over the row's canonical bytes:
//!   its fields in file column order, trimmed, joined with the
//!   unit separator (`\x1f`). Two runs over the same file produce
//!   the same row checksums.
//! * **Imported-rows checksum** — SHA-256 over the concatenation
//!   of the imported rows' checksums (in file order), so the set
//!   of imported rows is itself verifiable without listing them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// SHA-256 hex of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Canonical bytes for one CSV row: fields in file column order,
/// trimmed, joined with `\x1f` (unit separator — a byte that
/// cannot appear in a trimmed field without being part of it,
/// since fields are joined, not escaped).
pub fn canonical_row_bytes(fields: &[String]) -> Vec<u8> {
    fields
        .iter()
        .map(|field| field.trim())
        .collect::<Vec<&str>>()
        .join("\u{1f}")
        .into_bytes()
}

/// SHA-256 hex of a row's canonical bytes.
pub fn row_checksum(fields: &[String]) -> String {
    sha256_hex(&canonical_row_bytes(fields))
}

/// Deterministic UUID from a seed string: the first 16 bytes of
/// its SHA-256, with the version nibble set to 5 (name-based) and
/// the RFC 4122 variant bits set. Used for ids that must be
/// stable across re-imports — the same legacy row always maps to
/// the same id, so a re-import finds the existing row instead of
/// duplicating it.
pub fn uuid_from_seed(seed: &str) -> Uuid {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes: [u8; 16] = digest[..16].try_into().expect("sha256 is 32 bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// What happened to one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowStatus {
    Imported,
    Skipped,
    Failed,
}

/// One row's outcome in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowOutcome {
    /// 1-based data-row number (header is not counted).
    pub row_number: usize,
    pub status: RowStatus,
    /// Why a row was skipped or failed (`None` when imported).
    pub reason: Option<String>,
    /// SHA-256 of the row's canonical bytes.
    pub sha256: String,
}

/// One file's outcome in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManifest {
    pub file_name: String,
    /// `customers` | `products` | `formulas` | `ar_balances` |
    /// `inventory` | `unknown`.
    pub kind: String,
    /// SHA-256 of the raw file bytes (`None` when the file could
    /// not be read at all).
    pub file_sha256: Option<String>,
    /// A file-level failure (unreadable file, unparseable CSV,
    /// unknown kind) — when set, no rows were processed.
    pub file_error: Option<String>,
    pub rows_read: usize,
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
    /// SHA-256 over the concatenation of the imported rows'
    /// checksums, in file order (`None` when nothing imported).
    pub imported_rows_sha256: Option<String>,
    pub rows: Vec<RowOutcome>,
}

/// The whole import run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportManifest {
    pub imported_at: DateTime<Utc>,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub files: Vec<FileManifest>,
    pub total_rows_read: usize,
    pub total_imported: usize,
    pub total_skipped: usize,
    pub total_failed: usize,
}

impl ImportManifest {
    /// True when every row of every file imported (no skips, no
    /// failures) — the exit-code criterion for the CLI.
    pub fn is_clean(&self) -> bool {
        self.total_skipped == 0 && self.total_failed == 0
    }
}

impl FileManifest {
    pub(crate) fn new(file_name: String, kind: String, file_sha256: Option<String>) -> Self {
        FileManifest {
            file_name,
            kind,
            file_sha256,
            file_error: None,
            rows_read: 0,
            imported: 0,
            skipped: 0,
            failed: 0,
            imported_rows_sha256: None,
            rows: Vec::new(),
        }
    }

    pub(crate) fn record(&mut self, outcome: RowOutcome) {
        match outcome.status {
            RowStatus::Imported => self.imported += 1,
            RowStatus::Skipped => self.skipped += 1,
            RowStatus::Failed => self.failed += 1,
        }
        self.rows_read += 1;
        self.rows.push(outcome);
    }

    /// Finalize the imported-rows checksum once all rows are
    /// recorded.
    pub(crate) fn finish(&mut self) {
        let mut hasher = Sha256::new();
        let mut any = false;
        for row in &self.rows {
            if row.status == RowStatus::Imported {
                hasher.update(row.sha256.as_bytes());
                any = true;
            }
        }
        if any {
            self.imported_rows_sha256 = Some(format!("{:x}", hasher.finalize()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_bytes_trim_and_join() {
        let bytes = canonical_row_bytes(&[" a ".to_string(), "b".to_string()]);
        assert_eq!(bytes, b"a\x1fb");
        assert_eq!(
            row_checksum(&[" a ".to_string()]),
            row_checksum(&["a".to_string()])
        );
    }

    #[test]
    fn uuid_from_seed_is_stable_and_versioned() {
        let first = uuid_from_seed("department:PAINT");
        let second = uuid_from_seed("department:PAINT");
        assert_eq!(first, second);
        assert_ne!(first, uuid_from_seed("department:OTHER"));
        assert_eq!(first.get_version_num(), 5);
        assert_eq!(first.get_variant(), uuid::Variant::RFC4122);
    }

    #[test]
    fn file_manifest_counts_and_imported_checksum() {
        let mut manifest = FileManifest::new("x.csv".to_string(), "customers".to_string(), None);
        let imported = RowOutcome {
            row_number: 1,
            status: RowStatus::Imported,
            reason: None,
            sha256: "aa".to_string(),
        };
        let skipped = RowOutcome {
            row_number: 2,
            status: RowStatus::Skipped,
            reason: Some("duplicate".to_string()),
            sha256: "bb".to_string(),
        };
        manifest.record(imported);
        manifest.record(skipped);
        manifest.finish();
        assert_eq!(manifest.rows_read, 2);
        assert_eq!(manifest.imported, 1);
        assert_eq!(manifest.skipped, 1);
        assert_eq!(manifest.failed, 0);
        // Only the imported row's checksum feeds the aggregate.
        assert_eq!(
            manifest.imported_rows_sha256.as_deref(),
            Some(sha256_hex(b"aa").as_str())
        );
    }
}
