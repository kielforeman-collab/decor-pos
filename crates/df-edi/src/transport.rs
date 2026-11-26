//! Vendor transports (Phase 4, v0.5).
//!
//! ## What is real and what is a stand-in — read this first
//!
//! [`VendorTransport`] is the trait a vendor exchange uses:
//! `send` an outbound file, `list_inbox` / `receive` inbound
//! files, `archive_inbox` a file once it has been processed.
//!
//! The only implementation in this crate is
//! [`LocalSftpStandIn`]: a local drop folder (`outbox/`,
//! `inbox/`, `processed/` under one root directory). It is a
//! **stand-in for a real SFTP client, not an SFTP client**:
//!
//! * **Real:** files are actually written, listed, read and moved
//!   on disk; filenames are validated (no path separators or `..`);
//!   `send` fails loudly on I/O errors instead of pretending the
//!   file left the building.
//! * **Simulated / absent:** there is no SSH transport, no host-key
//!   verification, no username/password or key authentication, no
//!   remote server, no encryption in transit, no resume/retry, and
//!   no delivery confirmation from a vendor. A file in `outbox/`
//!   has only been staged locally — it has not reached any vendor.
//!
//! A real SFTP implementation would implement the same trait over
//! an SSH session (same file names, same `send`/`receive`
//! contract) and would add host-key checking and credentials from
//! the secure store — never hardcoded here. AS2 over the network
//! is likewise out of scope: see [`crate::as2`] for the
//! message/MDN data model, which is real MIME packaging with
//! honestly-`NotSupported` crypto.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{EdiError, Result};

/// A vendor file exchange: send outbound files, pick up inbound
/// files, archive them once processed.
///
/// Implementations must fail loudly on I/O errors and must never
/// claim a file was delivered to a vendor when it was only staged
/// locally.
pub trait VendorTransport {
    /// Short name for logs / `edi_transport_records.transport`
    /// (e.g. `"local-sftp-stand-in"`).
    fn transport_name(&self) -> &str;

    /// Stage an outbound file for the vendor. Returns the path the
    /// file was written to.
    fn send(&self, file_name: &str, contents: &str) -> Result<PathBuf>;

    /// File names waiting in the inbox, sorted.
    fn list_inbox(&self) -> Result<Vec<String>>;

    /// Read one inbox file by name.
    fn receive(&self, file_name: &str) -> Result<String>;

    /// Move an inbox file to the processed archive after it has
    /// been parsed and stored.
    fn archive_inbox(&self, file_name: &str) -> Result<()>;
}

/// Validate a transport file name: a bare file name, no path
/// separators, no parent-directory escape, non-empty.
pub fn validate_file_name(file_name: &str) -> Result<()> {
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Err(EdiError::Parse(format!(
            "unsafe transport file name {file_name:?}"
        )));
    }
    Ok(())
}

/// Local drop-folder stand-in for a real SFTP client.
///
/// Layout under `root`:
///
/// ```text
/// <root>/
///   outbox/     — files staged for the vendor (send() writes here)
///   inbox/      — files dropped by the vendor / test harness (receive() reads here)
///   processed/  — inbox files moved here by archive_inbox()
/// ```
///
/// See the module docs for exactly what is real and what is
/// simulated. No network, no SSH, no vendor connection.
#[derive(Debug, Clone)]
pub struct LocalSftpStandIn {
    root: PathBuf,
}

impl LocalSftpStandIn {
    /// Create the stand-in for a root directory. Directories are
    /// created lazily by the first `send`/`receive`/`list` call, so
    /// constructing one never touches the disk.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn outbox_dir(&self) -> PathBuf {
        self.root.join("outbox")
    }

    fn inbox_dir(&self) -> PathBuf {
        self.root.join("inbox")
    }

    fn processed_dir(&self) -> PathBuf {
        self.root.join("processed")
    }

    fn ensure_dirs(&self) -> Result<()> {
        for dir in [self.outbox_dir(), self.inbox_dir(), self.processed_dir()] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

impl VendorTransport for LocalSftpStandIn {
    fn transport_name(&self) -> &str {
        "local-sftp-stand-in"
    }

    fn send(&self, file_name: &str, contents: &str) -> Result<PathBuf> {
        validate_file_name(file_name)?;
        self.ensure_dirs()?;
        let path = self.outbox_dir().join(file_name);
        fs::write(&path, contents)?;
        Ok(path)
    }

    fn list_inbox(&self) -> Result<Vec<String>> {
        self.ensure_dirs()?;
        let mut names = Vec::new();
        for entry in fs::read_dir(self.inbox_dir())? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(name) = entry.file_name().to_str() {
                    names.push(name.to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    fn receive(&self, file_name: &str) -> Result<String> {
        validate_file_name(file_name)?;
        self.ensure_dirs()?;
        Ok(fs::read_to_string(self.inbox_dir().join(file_name))?)
    }

    fn archive_inbox(&self, file_name: &str) -> Result<()> {
        validate_file_name(file_name)?;
        self.ensure_dirs()?;
        fs::rename(
            self.inbox_dir().join(file_name),
            self.processed_dir().join(file_name),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("df-edi-transport-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn drop_folder_send_receive_archive_round_trip() {
        let root = temp_root("roundtrip");
        let transport = LocalSftpStandIn::new(&root);
        let sent = transport.send("850-PO-1.x12", "ST*850*1~").unwrap();
        assert!(sent.starts_with(root.join("outbox")));
        assert_eq!(fs::read_to_string(sent).unwrap(), "ST*850*1~");

        // Simulate a vendor drop into the inbox.
        fs::create_dir_all(root.join("inbox")).unwrap();
        fs::write(root.join("inbox").join("855-PO-1.x12"), "ST*855*1~").unwrap();
        assert_eq!(
            transport.list_inbox().unwrap(),
            vec!["855-PO-1.x12".to_string()]
        );
        assert_eq!(transport.receive("855-PO-1.x12").unwrap(), "ST*855*1~");
        transport.archive_inbox("855-PO-1.x12").unwrap();
        assert!(transport.list_inbox().unwrap().is_empty());
        assert!(root.join("processed").join("855-PO-1.x12").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unsafe_file_names_are_rejected() {
        let transport = LocalSftpStandIn::new(temp_root("unsafe"));
        assert!(transport.send("../escape.x12", "x").is_err());
        assert!(transport.send("sub/file.x12", "x").is_err());
        assert!(transport.receive("..\\escape.x12").is_err());
        assert!(validate_file_name("850-PO-1.x12").is_ok());
    }

    #[test]
    fn missing_inbox_file_errors() {
        let transport = LocalSftpStandIn::new(temp_root("missing"));
        assert!(transport.receive("no-such-file.x12").is_err());
    }
}
