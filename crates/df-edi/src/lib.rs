//! `df-edi` — file-based vendor EDI for decor-pos (Phase 2, v0.3).
//!
//! Spec §1 (Tinter & Vendor EDI) and §7 Phase 2 row, pulled forward
//! from Phase 4 in file-based form: this crate builds and parses
//! ANSI X12 documents and CSV vendor files from local file drops.
//! There is **no** SFTP/AS2 transport and no live vendor endpoint
//! here — those are Phase 4. Benjamin Moore COLORx is deferred
//! post-v1.0 (spec §9) and appears nowhere in this crate.
//!
//! ## What is implemented
//!
//! * [`purchase_order::generate_purchase_orders`] — purchase orders
//!   from low-stock inputs (one PO per vendor, exact `Decimal` math).
//! * [`purchase_order::purchase_order_to_x12_850`] /
//!   [`purchase_order::parse_850`] — X12 850 builder (real `ISA`/`GS`/
//!   `ST`/`BEG`/`PO1`/`SE`/`GE`/`IEA` segments with real control
//!   numbers and a correct `SE` segment count) and parser; the
//!   round-trip preserves PO number, vendor, date and lines. Fields
//!   X12 cannot carry (`product_id`, `store_id`) come back as nil
//!   UUIDs — see `parse_850`.
//! * [`purchase_order::purchase_order_to_csv`] — CSV PO form.
//! * [`intake::parse_856`] / [`intake::parse_810`] — X12 ASN and
//!   invoice parsing with errors on malformed segments.
//! * [`intake::parse_csv_intake`] — CSV ASN/invoice intake (one
//!   document per file).
//! * [`catalog::import_vendor_catalog_csv`] — vendor catalog import
//!   with per-row validation errors (one bad row never fails the
//!   file).
//! * [`repos`] — `purchase_orders`, `purchase_order_lines` and
//!   `edi_documents` repositories over [`df_db::AnyPool`]
//!   (migration `0004_phase2_edi`, both dialects). Repos live in this
//!   crate; `df-db` itself is untouched.
//!
//! ## What is NOT implemented (Phase 4)
//!
//! X12 855 acknowledgments and 832 catalogs (only stored as
//! `edi_documents` rows via [`repos::EdiDocType`]), SFTP/AS2/REST
//! vendor transports, and the `EdiVendorPlugin` poll/send calls —
//! those go through `df-plugin-sdk` once a vendor plugin exists.

pub mod catalog;
pub mod csv;
pub mod error;
pub mod intake;
pub mod purchase_order;
pub mod repos;
pub mod x12;

pub use catalog::{CatalogImport, CatalogItem, CatalogRowError, import_vendor_catalog_csv};
pub use error::{EdiError, Result};
pub use intake::{
    CsvIntake, Invoice, InvoiceLine, Shipment, ShipmentLine, parse_810, parse_856, parse_csv_intake,
};
pub use purchase_order::{
    LowStockItem, PoStatus, PurchaseOrder, PurchaseOrderLine, generate_purchase_orders, parse_850,
    purchase_order_to_csv, purchase_order_to_x12_850,
};
pub use repos::{EdiDirection, EdiDocType, EdiDocument};
pub use x12::{Segment, parse_document};
