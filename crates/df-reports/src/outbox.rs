//! Scheduled report delivery: a file-based email outbox (Phase 6,
//! v1.0; spec §7).
//!
//! There is **no live SMTP send** here — deliberately. A scheduled
//! email job renders a report (Typst PDF and/or CSV) and records
//! the send intent in an outbox directory on disk:
//!
//! * the rendered file, e.g. `kpi-2026-01-10-<id>.pdf`;
//! * `manifest.json` — one [`EmailDelivery`] per queued file,
//!   recording the recipient, subject, rendered file name, its
//!   SHA-256, and the scheduled time.
//!
//! A mail relay (or an operator) picks the manifest up and sends
//! it; until a relay exists the manifest is the honest record of
//! what *would* have been sent, and `status` stays `"queued"`.
//! Nothing in this module opens a socket.
//!
//! The manifest is read-modify-written as one JSON array. That is
//! safe for the single-process edge daemon / `df-server jobs run`
//! writer; concurrent writers from two processes are not guarded
//! (no file lock) — run one delivery writer per outbox directory.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use uuid::Uuid;

use crate::error::ReportError;

/// One queued report email: the send intent plus the rendered
/// file it carries. `status` is always `"queued"` — delivery is a
/// mail relay's job, not this crate's (no SMTP here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmailDelivery {
    pub id: Uuid,
    pub report_kind: String,
    pub recipient: String,
    pub subject: String,
    pub file_name: String,
    pub file_sha256: String,
    pub scheduled_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub status: String,
}

fn manifest_path(outbox_dir: &Path) -> PathBuf {
    outbox_dir.join("manifest.json")
}

/// SHA-256 hex of rendered bytes — the value the manifest stores
/// so a relay (or an auditor) can verify the file it sends.
pub fn file_sha256(bytes: &[u8]) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Read the outbox manifest (`manifest.json` in `outbox_dir`).
/// An absent manifest reads as an empty outbox; a present but
/// malformed one is an error, never silently dropped.
pub fn read_outbox_manifest(outbox_dir: &Path) -> Result<Vec<EmailDelivery>, ReportError> {
    let path = manifest_path(outbox_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            ReportError::InvalidInput(format!("outbox manifest {path:?} is not valid JSON: {e}"))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(ReportError::Io(e)),
    }
}

/// Queue one rendered report file for email delivery: write the
/// file into `outbox_dir` (created if needed), then append the
/// delivery record to `manifest.json` and return it.
///
/// `file_name` must be a bare file name (no separators, no `..`)
/// — a path in it is refused rather than written outside the
/// outbox.
pub fn deliver_to_outbox(
    outbox_dir: &Path,
    report_kind: &str,
    recipient: &str,
    subject: &str,
    scheduled_at: DateTime<Utc>,
    file_name: &str,
    bytes: &[u8],
) -> Result<EmailDelivery, ReportError> {
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Err(ReportError::InvalidInput(format!(
            "outbox file_name must be a bare file name, got {file_name:?}"
        )));
    }
    std::fs::create_dir_all(outbox_dir)?;
    std::fs::write(outbox_dir.join(file_name), bytes)?;
    let mut deliveries = read_outbox_manifest(outbox_dir)?;
    let delivery = EmailDelivery {
        id: Uuid::now_v7(),
        report_kind: report_kind.to_string(),
        recipient: recipient.to_string(),
        subject: subject.to_string(),
        file_name: file_name.to_string(),
        file_sha256: file_sha256(bytes),
        scheduled_at,
        created_at: Utc::now(),
        status: "queued".to_string(),
    };
    deliveries.push(delivery.clone());
    std::fs::write(
        manifest_path(outbox_dir),
        serde_json::to_string_pretty(&deliveries).map_err(|e| {
            ReportError::InvalidInput(format!("outbox manifest did not serialise: {e}"))
        })?,
    )?;
    Ok(delivery)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_outbox() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("df-outbox-test-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn outbox_writes_file_and_manifest_entry() {
        let dir = temp_outbox();
        let bytes = b"%PDF-1.4 fake pdf bytes";
        let scheduled_at = Utc::now();
        let delivery = deliver_to_outbox(
            &dir,
            "kpi-dashboard",
            "manager@example.com",
            "KPI dashboard 2026-01-10",
            scheduled_at,
            "kpi-2026-01-10.pdf",
            bytes,
        )
        .unwrap();
        assert_eq!(delivery.report_kind, "kpi-dashboard");
        assert_eq!(delivery.recipient, "manager@example.com");
        assert_eq!(delivery.file_name, "kpi-2026-01-10.pdf");
        assert_eq!(delivery.file_sha256, file_sha256(bytes));
        assert_eq!(delivery.scheduled_at, scheduled_at);
        // The status is honest: queued, not sent — no SMTP exists.
        assert_eq!(delivery.status, "queued");
        assert_eq!(
            std::fs::read(dir.join("kpi-2026-01-10.pdf")).unwrap(),
            bytes
        );

        let manifest = read_outbox_manifest(&dir).unwrap();
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0], delivery);

        // A second delivery appends; the first entry is untouched.
        let second = deliver_to_outbox(
            &dir,
            "eco-remittance",
            "accounts@example.com",
            "Eco remittance Feb 2026",
            scheduled_at,
            "eco-2026-02.csv",
            b"country_code,region_code\n",
        )
        .unwrap();
        let manifest = read_outbox_manifest(&dir).unwrap();
        assert_eq!(manifest.len(), 2);
        assert_eq!(manifest[0], delivery);
        assert_eq!(manifest[1], second);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outbox_rejects_path_traversal_file_names() {
        let dir = temp_outbox();
        for bad in ["../escape.pdf", "sub/file.pdf", ""] {
            let err = deliver_to_outbox(
                &dir,
                "kpi-dashboard",
                "a@example.com",
                "subject",
                Utc::now(),
                bad,
                b"x",
            )
            .unwrap_err();
            assert!(err.to_string().contains("bare file name"));
        }
        // Nothing was written outside the outbox.
        assert!(read_outbox_manifest(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_outbox_reads_as_no_deliveries() {
        let dir = temp_outbox();
        assert!(read_outbox_manifest(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
