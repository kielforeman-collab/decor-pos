//! Vendor EDI on the edge (Phase 4, v0.5; spec §6.1/§7): PO send
//! (X12 850, file-drop transport), ASN/invoice/ack/catalog intake
//! (856/810/855/832, X12 or CSV), the 3-way match report, and
//! vendor catalog import.
//!
//! Everything here is built on the real `df-edi` crate:
//! `purchase_order_to_x12_850`, `parse_856` / `parse_810` /
//! `parse_855` / `parse_832`, `parse_csv_intake`,
//! `import_vendor_catalog_csv`, `match_three_way`,
//! `build_price_apply_report`, `apply_acknowledgment_to_po`, the
//! `LocalSftpStandIn` transport and the `edi_documents` /
//! `po_acknowledgments` / `invoice_match_reports` /
//! `price_catalog_applications` / `edi_transport_records` repos.
//! The edge adds the HTTP-facing rules: send stores the outbound
//! 850 as an `edi_documents` row, flips the PO to `sent` and
//! stages the file through the transport; intake stores the raw
//! document plus its parsed JSON, links it to the PO the vendor
//! echoes, and runs the ack / catalog follow-ups; the 3-way match
//! is the `df-edi` report over the stored PO, latest ASN and
//! latest invoice, persisted when both documents are present.
//!
//! Honest limits, stated here and in the README — the only
//! transport is the `LocalSftpStandIn` file drop: a file in
//! `outbox/` has only been staged locally, there is no SSH, no
//! host-key check, no credentials, no remote server and no
//! delivery confirmation from any vendor. AS2 is real MIME
//! packaging with honestly-`NotSupported` crypto and no network.
//! The 832 apply report is a **review** record
//! (`pending_review`): marking it applied records the review
//! decision only — no product prices are rewritten by this
//! endpoint. Applying an 855 ack is likewise a report, never a
//! mutation of the stored PO. No live vendor endpoint is ever
//! called. Benjamin Moore COLORx is deferred post-v1.0 (spec §9)
//! and appears nowhere here.

use chrono::Utc;
use df_db::DbError;
use df_edi::{
    AckApplication, CatalogItem, CsvIntake, CurrentPrice, EdiDirection, EdiDocType, EdiDocument,
    Invoice, LocalSftpStandIn, PriceCatalog, Shipment, VendorTransport,
};

/// Re-exported so `http` can name the rows these endpoints
/// return. (The `pub use` itself brings the names into this
/// module's scope, so the methods below use them directly.)
pub use df_edi::{
    InvoiceMatchReport, InvoiceMatchReportRow, PoAcknowledgmentRow, PriceCatalogApplicationRow,
    TransportRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

/// X12 sender id used in the 850 envelope (ISA/GS).
pub const EDI_SENDER_ID: &str = "DECORPOS";

// --- request / response types -------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendPoRequest {
    /// Root folder for the transport stand-in (`outbox/` /
    /// `inbox/` / `processed/` are created under it). When
    /// absent the 850 is stored + returned but no file is
    /// staged.
    pub out_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendPoResponse {
    pub po_id: Uuid,
    pub po_number: String,
    pub doc_id: Uuid,
    pub x12_850: String,
    /// Path the 850 was staged at (`{out_dir}/outbox/{po_number}.850.x12`),
    /// or `None` when no `out_dir` was given.
    pub written_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntakeRequest {
    /// `856`, `810`, `855`, `832` or `csv` (CSV ASN/invoice — the
    /// parsed type is detected from the rows).
    pub doc_type: String,
    pub raw_text: String,
    pub vendor_id: String,
    pub related_po_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntakeResponse {
    pub doc_id: Uuid,
    pub doc_type: String,
    pub parsed: bool,
    pub related_po_id: Option<Uuid>,
    pub parsed_json: Value,
    pub note: String,
    /// The ack-application report, for an 855 intake: what the
    /// vendor accepted / rejected / changed. The stored PO is
    /// never mutated by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_application: Option<AckApplication>,
    /// The price-apply report id, for an 832 intake. The report
    /// is `pending_review` — marking it applied is a separate,
    /// reviewed step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_application_id: Option<Uuid>,
}

/// Mark a price-catalog application applied after review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkPriceApplicationAppliedRequest {
    pub reviewed_by: String,
}

/// One rejected vendor catalog row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogImportError {
    pub row: usize,
    pub message: String,
}

/// The result of a vendor catalog CSV import: valid items plus
/// every bad row (one bad row never fails the file).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogImportResponse {
    pub imported: usize,
    pub items: Vec<CatalogItem>,
    pub errors: Vec<CatalogImportError>,
}

// --- helpers ------------------------------------------------------------------

impl Edge {
    /// Find a stored PO by its number for a vendor, for linking an
    /// intake document the vendor echoes the PO number on.
    async fn find_po_by_number(
        &self,
        vendor_id: &str,
        po_number: &str,
    ) -> Result<Option<Uuid>, EdgeError> {
        let pos = df_edi::repos::list_purchase_orders_for_vendor(self.pool(), vendor_id).await?;
        Ok(pos
            .into_iter()
            .find(|po| po.po_number == po_number)
            .map(|po| po.id))
    }

    /// The store's current prices, by product SKU, for an 832
    /// apply report. Cost is the product's standard cost and list
    /// price its list price; SKUs the store does not carry simply
    /// have no current price.
    async fn current_prices(&self) -> Result<Vec<CurrentPrice>, EdgeError> {
        let products = df_db::list_products_for_org(self.pool(), self.config().org_id).await?;
        Ok(products
            .into_iter()
            .map(|product| CurrentPrice {
                vendor_sku: product.sku,
                cost: product.standard_cost,
                list_price: product.list_price,
            })
            .collect())
    }

    // --- PO send ------------------------------------------------------------

    /// Send a purchase order as X12 850: build the document from
    /// the stored PO, store it as an outbound `edi_documents` row,
    /// flip the PO to `sent`, and — when `out_dir` is given —
    /// stage `{po_number}.850.x12` through the
    /// `LocalSftpStandIn` transport (`{out_dir}/outbox/`) and
    /// record the send in `edi_transport_records`. The file has
    /// only been staged locally — it has not reached any vendor.
    pub async fn send_purchase_order(
        &self,
        po_id: Uuid,
        out_dir: Option<String>,
    ) -> Result<SendPoResponse, EdgeError> {
        let po = df_edi::repos::get_purchase_order(self.pool(), po_id).await?;
        if po.status != df_edi::PoStatus::Draft {
            return Err(EdgeError::InvalidInput(format!(
                "PO {} is {} — only a draft PO can be sent",
                po.po_number,
                po.status.as_str()
            )));
        }
        // X12 interchange control number: seconds since epoch, kept
        // inside 9 digits. Unique enough for file drops; a vendor
        // plugin that needs strict sequencing owns its own counter.
        let control = (Utc::now().timestamp() % 1_000_000_000).max(1) as u32;
        let x12 = df_edi::purchase_order_to_x12_850(&po, EDI_SENDER_ID, "", control);
        let doc = EdiDocument {
            id: Uuid::now_v7(),
            doc_type: EdiDocType::Po850,
            direction: EdiDirection::Outbound,
            vendor_id: po.vendor_id.clone(),
            related_po_id: Some(po.id),
            raw_text: x12.clone(),
            parsed_json: serde_json::to_value(&po)
                .map_err(|err| EdgeError::InvalidInput(format!("PO does not serialise: {err}")))?,
            received_at: Utc::now(),
        };
        df_edi::repos::insert_edi_document(self.pool(), &doc).await?;
        sqlx::query("UPDATE purchase_orders SET status = $1 WHERE id = $2")
            .bind("sent")
            .bind(po_id.to_string())
            .execute(self.pool())
            .await
            .map_err(DbError::Sqlx)?;
        let written_to = match out_dir {
            Some(dir) => {
                let transport = LocalSftpStandIn::new(dir);
                let file_name = format!("{}.850.x12", po.po_number);
                let path = transport.send(&file_name, &x12)?;
                df_edi::repos::insert_transport_record(
                    self.pool(),
                    &TransportRecord {
                        id: Uuid::now_v7(),
                        vendor_id: po.vendor_id.clone(),
                        direction: EdiDirection::Outbound,
                        transport: transport.transport_name().to_string(),
                        file_name,
                        doc_type: Some(EdiDocType::Po850),
                        status: "sent".to_string(),
                        error: None,
                        created_at: Utc::now(),
                    },
                )
                .await?;
                Some(path.to_string_lossy().to_string())
            }
            None => None,
        };
        Ok(SendPoResponse {
            po_id: po.id,
            po_number: po.po_number,
            doc_id: doc.id,
            x12_850: x12,
            written_to,
        })
    }

    // --- intake ---------------------------------------------------------------

    /// Intake one vendor document. 856/810 (X12 or CSV), 855 and
    /// 832 are all parsed with the real `df-edi` parsers and
    /// linked to the PO the vendor echoes. An 855 additionally
    /// stores the acknowledgment and returns the
    /// ack-application report (the PO itself is never mutated);
    /// an 832 additionally builds and stores a price-apply
    /// report in `pending_review` (nothing is repriced here).
    pub async fn intake_edi(&self, body: IntakeRequest) -> Result<IntakeResponse, EdgeError> {
        if body.vendor_id.trim().is_empty() {
            return Err(EdgeError::InvalidInput(
                "vendor_id must not be empty".to_string(),
            ));
        }
        if body.raw_text.trim().is_empty() {
            return Err(EdgeError::InvalidInput(
                "raw_text must not be empty".to_string(),
            ));
        }
        let mut parsed_ack: Option<df_edi::PoAcknowledgment> = None;
        let mut parsed_catalog: Option<PriceCatalog> = None;
        let (doc_type, parsed_json, parsed, note, po_number) = match body.doc_type.as_str() {
            "856" => {
                let shipment = df_edi::parse_856(&body.raw_text)?;
                let po_number = shipment.po_number.clone();
                (
                    EdiDocType::Asn856,
                    serde_json::to_value(&shipment).map_err(|err| {
                        EdgeError::InvalidInput(format!("ASN does not serialise: {err}"))
                    })?,
                    true,
                    "ASN parsed".to_string(),
                    po_number,
                )
            }
            "810" => {
                let invoice = df_edi::parse_810(&body.raw_text)?;
                let po_number = invoice.po_number.clone();
                (
                    EdiDocType::Invoice810,
                    serde_json::to_value(&invoice).map_err(|err| {
                        EdgeError::InvalidInput(format!("invoice does not serialise: {err}"))
                    })?,
                    true,
                    "invoice parsed".to_string(),
                    po_number,
                )
            }
            "csv" => match df_edi::parse_csv_intake(&body.raw_text)? {
                CsvIntake::Shipment(shipment) => {
                    let po_number = shipment.po_number.clone();
                    (
                        EdiDocType::Asn856,
                        serde_json::to_value(&shipment).map_err(|err| {
                            EdgeError::InvalidInput(format!("ASN does not serialise: {err}"))
                        })?,
                        true,
                        "CSV ASN parsed".to_string(),
                        po_number,
                    )
                }
                CsvIntake::Invoice(invoice) => {
                    let po_number = invoice.po_number.clone();
                    (
                        EdiDocType::Invoice810,
                        serde_json::to_value(&invoice).map_err(|err| {
                            EdgeError::InvalidInput(format!("invoice does not serialise: {err}"))
                        })?,
                        true,
                        "CSV invoice parsed".to_string(),
                        po_number,
                    )
                }
            },
            "855" => {
                let ack = df_edi::parse_855(&body.raw_text)?;
                let po_number = Some(ack.po_number.clone());
                parsed_ack = Some(ack.clone());
                (
                    EdiDocType::Ack855,
                    serde_json::to_value(&ack).map_err(|err| {
                        EdgeError::InvalidInput(format!("ack does not serialise: {err}"))
                    })?,
                    true,
                    "855 acknowledgment parsed".to_string(),
                    po_number,
                )
            }
            "832" => {
                let catalog = df_edi::parse_832(&body.raw_text)?;
                parsed_catalog = Some(catalog.clone());
                (
                    EdiDocType::Catalog832,
                    serde_json::to_value(&catalog).map_err(|err| {
                        EdgeError::InvalidInput(format!("catalog does not serialise: {err}"))
                    })?,
                    true,
                    "832 catalog parsed — apply report pending review".to_string(),
                    None,
                )
            }
            other => {
                return Err(EdgeError::InvalidInput(format!(
                    "doc_type must be 856, 810, 855, 832 or csv — got {other:?}"
                )));
            }
        };
        let mut related_po_id = body.related_po_id;
        if related_po_id.is_none()
            && let Some(po_number) = po_number
        {
            related_po_id = self.find_po_by_number(&body.vendor_id, &po_number).await?;
        }
        if let Some(po_id) = related_po_id {
            // The link must point at a real PO; a typo'd id is a
            // 400, not a dangling reference.
            df_edi::repos::get_purchase_order(self.pool(), po_id).await?;
        }
        let doc = EdiDocument {
            id: Uuid::now_v7(),
            doc_type,
            direction: EdiDirection::Inbound,
            vendor_id: body.vendor_id,
            related_po_id,
            raw_text: body.raw_text,
            parsed_json: parsed_json.clone(),
            received_at: Utc::now(),
        };
        df_edi::repos::insert_edi_document(self.pool(), &doc).await?;
        // 855 follow-up: store the acknowledgment and apply it
        // (as a report — the PO is never mutated). The ack must
        // answer a PO we hold; an ack for an unknown PO number
        // is a 400, not a silent drop.
        let mut ack_application = None;
        if let Some(ack) = &parsed_ack {
            let po_id = related_po_id.ok_or_else(|| {
                EdgeError::InvalidInput(format!(
                    "855 answers PO {:?} but no matching PO is stored for this vendor",
                    ack.po_number
                ))
            })?;
            let po = df_edi::repos::get_purchase_order(self.pool(), po_id).await?;
            let application = df_edi::apply_acknowledgment_to_po(&po, ack)?;
            df_edi::repos::insert_po_acknowledgment(self.pool(), po.id, ack).await?;
            ack_application = Some(application);
        }
        // 832 follow-up: build the price-apply report against
        // the store's current prices and store it pending review.
        // Nothing is repriced here.
        let mut price_application_id = None;
        if let Some(catalog) = &parsed_catalog {
            let current = self.current_prices().await?;
            let application = df_edi::build_price_apply_report(catalog, &current);
            df_edi::repos::insert_price_catalog_application(self.pool(), &application, None)
                .await?;
            price_application_id = Some(application.id);
        }
        Ok(IntakeResponse {
            doc_id: doc.id,
            doc_type: doc_type.as_str().to_string(),
            parsed,
            related_po_id,
            parsed_json,
            note,
            ack_application,
            price_application_id,
        })
    }

    /// List EDI documents linked to a PO, newest first.
    pub async fn list_edi_documents(&self, po_id: Uuid) -> Result<Vec<EdiDocument>, EdgeError> {
        df_edi::repos::get_purchase_order(self.pool(), po_id).await?;
        Ok(df_edi::repos::list_edi_documents_for_po(self.pool(), po_id).await?)
    }

    /// Fetch one EDI document by id.
    pub async fn get_edi_document(&self, id: Uuid) -> Result<EdiDocument, EdgeError> {
        df_edi::repos::get_edi_document(self.pool(), id)
            .await
            .map_err(|err| match err {
                df_edi::EdiError::Parse(message) if message.ends_with("not found") => {
                    EdgeError::NotFound(message)
                }
                other => EdgeError::from(other),
            })
    }

    // --- acknowledgments ------------------------------------------------------

    /// Stored 855 acknowledgments for a PO, newest first.
    pub async fn list_po_acknowledgments(
        &self,
        po_id: Uuid,
    ) -> Result<Vec<df_edi::PoAcknowledgmentRow>, EdgeError> {
        df_edi::repos::get_purchase_order(self.pool(), po_id).await?;
        Ok(df_edi::repos::list_po_acknowledgments_for_po(self.pool(), po_id).await?)
    }

    // --- 3-way match ----------------------------------------------------------

    /// Compare one PO's ordered quantities and prices against the
    /// latest stored ASN (shipped) and invoice (billed) for that
    /// PO, via `df-edi::match_three_way`. A document whose echoed
    /// PO number is not this PO is a 400 — the caller must link
    /// the right documents. When both documents are stored the
    /// report is persisted (`invoice_match_reports`) so a
    /// reviewer can pull it back later; with a document missing
    /// the report is computed on the fly and not stored.
    pub async fn three_way_match(&self, po_id: Uuid) -> Result<InvoiceMatchReport, EdgeError> {
        let po = df_edi::repos::get_purchase_order(self.pool(), po_id).await?;
        let docs = df_edi::repos::list_edi_documents_for_po(self.pool(), po_id).await?;
        let asn_doc = docs.iter().find(|doc| doc.doc_type == EdiDocType::Asn856);
        let invoice_doc = docs
            .iter()
            .find(|doc| doc.doc_type == EdiDocType::Invoice810);
        let shipment: Shipment = match asn_doc {
            None => Shipment {
                shipment_id: String::new(),
                po_number: Some(po.po_number.clone()),
                ship_date: None,
                lines: vec![],
            },
            Some(doc) => {
                let shipment: Shipment =
                    serde_json::from_value(doc.parsed_json.clone()).map_err(|err| {
                        EdgeError::InvalidInput(format!(
                            "stored ASN {} has unparsable parsed_json: {err}",
                            doc.id
                        ))
                    })?;
                if let Some(number) = &shipment.po_number
                    && *number != po.po_number
                {
                    return Err(EdgeError::InvalidInput(format!(
                        "stored ASN {} answers PO {number:?}, not {:?}",
                        doc.id, po.po_number
                    )));
                }
                shipment
            }
        };
        let invoice: Invoice = match invoice_doc {
            None => Invoice {
                invoice_number: String::new(),
                po_number: Some(po.po_number.clone()),
                invoice_date: None,
                lines: vec![],
                total: None,
            },
            Some(doc) => {
                let invoice: Invoice =
                    serde_json::from_value(doc.parsed_json.clone()).map_err(|err| {
                        EdgeError::InvalidInput(format!(
                            "stored invoice {} has unparsable parsed_json: {err}",
                            doc.id
                        ))
                    })?;
                if let Some(number) = &invoice.po_number
                    && *number != po.po_number
                {
                    return Err(EdgeError::InvalidInput(format!(
                        "stored invoice {} answers PO {number:?}, not {:?}",
                        doc.id, po.po_number
                    )));
                }
                invoice
            }
        };
        let report = df_edi::match_three_way(&po, &shipment, &invoice);
        if asn_doc.is_some() && invoice_doc.is_some() {
            df_edi::repos::insert_invoice_match_report(self.pool(), po_id, &report).await?;
        }
        Ok(report)
    }

    /// Stored 3-way match reports for a PO, newest first.
    pub async fn list_match_reports(
        &self,
        po_id: Uuid,
    ) -> Result<Vec<df_edi::InvoiceMatchReportRow>, EdgeError> {
        df_edi::repos::get_purchase_order(self.pool(), po_id).await?;
        Ok(df_edi::repos::list_invoice_match_reports_for_po(self.pool(), po_id).await?)
    }

    // --- price-catalog applications -------------------------------------------

    /// Fetch one stored 832 price-apply report by id.
    pub async fn get_price_catalog_application(
        &self,
        id: Uuid,
    ) -> Result<PriceCatalogApplicationRow, EdgeError> {
        df_edi::repos::get_price_catalog_application(self.pool(), id)
            .await
            .map_err(|err| match err {
                df_edi::EdiError::Parse(message) if message.ends_with("not found") => {
                    EdgeError::NotFound(message)
                }
                other => EdgeError::from(other),
            })
    }

    /// Mark a price-apply report applied after review. This
    /// records the review decision only — it rewrites no product
    /// prices; the lines it proposes stay a report for the
    /// operator to apply through the catalog.
    pub async fn mark_price_application_applied(
        &self,
        id: Uuid,
        reviewed_by: &str,
    ) -> Result<PriceCatalogApplicationRow, EdgeError> {
        if reviewed_by.trim().is_empty() {
            return Err(EdgeError::InvalidInput(
                "reviewed_by must not be empty".to_string(),
            ));
        }
        let mut row = self.get_price_catalog_application(id).await?;
        row.application.mark_applied(reviewed_by);
        let applied_at = Utc::now();
        sqlx::query(
            "UPDATE price_catalog_applications SET status = $1, reviewed_by = $2, applied_at = $3 WHERE id = $4",
        )
        .bind(row.application.status.as_str())
        .bind(reviewed_by)
        .bind(applied_at.to_rfc3339())
        .bind(id.to_string())
        .execute(self.pool())
        .await
        .map_err(DbError::Sqlx)?;
        self.get_price_catalog_application(id).await
    }

    // --- transport records ----------------------------------------------------

    /// Transport audit trail for a vendor, newest first.
    pub async fn list_transport_records(
        &self,
        vendor_id: &str,
    ) -> Result<Vec<TransportRecord>, EdgeError> {
        if vendor_id.trim().is_empty() {
            return Err(EdgeError::InvalidInput(
                "vendor_id must not be empty".to_string(),
            ));
        }
        Ok(df_edi::repos::list_transport_records_for_vendor(self.pool(), vendor_id).await?)
    }

    // --- catalog import -------------------------------------------------------

    /// Import a vendor catalog CSV (the `df-edi` CSV importer):
    /// valid rows come back as items, bad rows as per-row errors.
    /// This is the CSV form of the catalog — the X12 832 apply
    /// step is the intake path above, which builds a review
    /// report and reprices nothing on its own.
    pub async fn import_vendor_catalog(
        &self,
        csv_text: &str,
        vendor_id: &str,
    ) -> Result<CatalogImportResponse, EdgeError> {
        if vendor_id.trim().is_empty() {
            return Err(EdgeError::InvalidInput(
                "vendor_id must not be empty".to_string(),
            ));
        }
        let result = df_edi::import_vendor_catalog_csv(csv_text);
        Ok(CatalogImportResponse {
            imported: result.items.len(),
            items: result.items,
            errors: result
                .errors
                .into_iter()
                .map(|err| CatalogImportError {
                    row: err.row,
                    message: err.message,
                })
                .collect(),
        })
    }
}
