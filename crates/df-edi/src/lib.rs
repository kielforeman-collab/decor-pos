//! `df-edi` — vendor EDI for decor-pos (Phase 4, v0.5).
//!
//! Spec §1 (Tinter & Vendor EDI), §6.2 (`EdiVendorPlugin`), §7
//! Phase 4 row: ANSI X12 documents and vendor file exchange, in
//! file-based form. Benjamin Moore COLORx is deferred post-v1.0
//! (spec §9) and appears nowhere in this crate.
//!
//! ## What is real
//!
//! * X12 850 / 855 / 856 / 810 / 832 builders and parsers with
//!   real envelopes, control numbers and correct `SE` counts —
//!   round-trips preserve PO numbers, lines, quantities and exact
//!   `Decimal` money. See [`purchase_order`], [`acknowledgment`],
//!   [`intake`], [`price_catalog`].
//! * [`matching::match_three_way`] — the three-way match (PO vs
//!   received vs invoiced) with exact per-line quantity and price
//!   variances. A variance is reported, never posted.
//! * [`price_catalog::build_price_apply_report`] — an 832 apply
//!   report with before/after prices and status `pending_review`.
//!   Nothing writes live prices; a human marks it applied.
//! * [`transport::LocalSftpStandIn`] — a real local drop folder
//!   (`outbox/` / `inbox/` / `processed/`): files are actually
//!   written, listed, read and archived on disk.
//! * [`as2`] — real AS2 MIME packaging and MDN data structures
//!   (build / render / parse round-trips).
//! * [`file_vendor::FileEdiVendorPlugin`] — the spec §6.2
//!   `EdiVendorPlugin` trait (in `df-plugin-sdk`) backed by the
//!   drop folder, for testing a vendor pack with no vendor.
//! * [`repos`] — `purchase_orders`, `purchase_order_lines`,
//!   `edi_documents` (migration `0004_phase2_edi`) plus the Phase 4
//!   tables (migration `0009_phase4_edi`, both dialects): PO
//!   acknowledgments, invoice match reports + lines, price
//!   catalog applications + lines, transport records.
//!
//! ## What is simulated or not implemented — read before trusting
//!
//! * **SFTP is a stand-in, not a client.** `LocalSftpStandIn`
//!   has no SSH, no host-key verification, no authentication, no
//!   remote server and no delivery confirmation. A file in
//!   `outbox/` has only been staged locally — it has not reached
//!   any vendor. A real SFTP implementation would implement the
//!   same [`transport::VendorTransport`] trait over SSH.
//! * **AS2 has no network and no crypto.** Messages are MIME
//!   text handed to a transport as a file; S/MIME signing and
//!   encryption return [`error::EdiError::NotSupported`] and are
//!   never faked. A built AS2 message is unsigned and unencrypted
//!   and says so in its headers.
//! * **No live vendor connection exists anywhere in this crate.**
//!   No vendor endpoint is contacted, no credentials are stored,
//!   and `submit_rebate_claim` on the file vendor answers
//!   `filed_locally` — staged, not vendor-confirmed.
//! * `HttpEdiVendorPlugin` (in `df-plugin-sdk`) speaks the trait
//!   over HTTP to a vendor plugin process; the plugin itself owns
//!   the vendor connection and must fail loudly when it cannot
//!   reach its vendor.

pub mod acknowledgment;
pub mod as2;
pub mod catalog;
pub mod csv;
pub mod error;
pub mod file_vendor;
pub mod intake;
pub mod matching;
pub mod price_catalog;
pub mod purchase_order;
pub mod repos;
pub mod transport;
pub mod x12;

pub use acknowledgment::{
    AckApplication, AckCode, AckLine, AckLineResult, PoAcknowledgment, apply_acknowledgment_to_po,
    parse_855, po_acknowledgment_to_x12_855,
};
pub use as2::{
    As2Message, As2Security, Mdn, MdnDisposition, build_as2_message, build_mdn, encrypt_payload,
    parse_as2_message, parse_mdn, render_as2_message, render_mdn, sign_payload,
};
pub use catalog::{CatalogImport, CatalogItem, CatalogRowError, import_vendor_catalog_csv};
pub use error::{EdiError, Result};
pub use file_vendor::FileEdiVendorPlugin;
pub use intake::{
    CsvIntake, Invoice, InvoiceLine, Shipment, ShipmentLine, invoice_to_x12_810, parse_810,
    parse_856, parse_csv_intake, shipment_to_x12_856,
};
pub use matching::{InvoiceMatchReport, MatchLine, MatchStatus, VarianceKind, match_three_way};
pub use price_catalog::{
    ApplicationStatus, AppliedPriceLine, CatalogAction, CurrentPrice, PriceApplyResult,
    PriceCatalog, PriceCatalogApplication, PriceCatalogItem, build_price_apply_report, parse_832,
    price_catalog_to_x12_832,
};
pub use purchase_order::{
    LowStockItem, PoStatus, PurchaseOrder, PurchaseOrderLine, generate_purchase_orders, parse_850,
    purchase_order_to_csv, purchase_order_to_x12_850,
};
pub use repos::{
    EdiDirection, EdiDocType, EdiDocument, InvoiceMatchReportRow, PoAcknowledgmentRow,
    PriceCatalogApplicationRow, TransportRecord,
};
pub use transport::{LocalSftpStandIn, VendorTransport, validate_file_name};
pub use x12::{Segment, parse_document};
