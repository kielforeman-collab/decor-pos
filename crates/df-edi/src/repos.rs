//! Repositories for the Phase 2 EDI tables (`0004_phase2_edi`).
//!
//! These live in `df-edi`, not in `df-db`: they operate on the
//! `df-db`-owned [`df_db::AnyPool`] type and follow the `df-db`
//! conventions — `$N` placeholders (sqlx's `Any` driver does NOT
//! rewrite `?`; a literal `?` syntax-errors on PostgreSQL),
//! ids / money / timestamps crossing the driver as text (UUID as
//! hyphenated text, money as an exact decimal string, timestamps as
//! RFC 3339). No `query!` macros, so the workspace builds with
//! `SQLX_OFFLINE=true` and no live database.
//!
//! PostgreSQL (HQ) enforces the FK graph; SQLite (edge) keeps the
//! edge-owned chains (`purchase_order_lines` / `edi_documents` →
//! `purchase_orders`) and omits FKs to HQ reference data.

use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::any::AnyRow;
use sqlx::{AnyPool, Row};
use uuid::Uuid;

use crate::acknowledgment::{AckCode, AckLine, PoAcknowledgment};
use crate::error::EdiError;
use crate::matching::{InvoiceMatchReport, MatchLine, MatchStatus, VarianceKind};
use crate::price_catalog::{
    ApplicationStatus, AppliedPriceLine, CatalogAction, PriceApplyResult, PriceCatalogApplication,
};
use crate::purchase_order::{PoStatus, PurchaseOrder, PurchaseOrderLine};

/// EDI document types stored in `edi_documents.doc_type` (spec §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdiDocType {
    /// 850 purchase order (outbound).
    Po850,
    /// 855 PO acknowledgment (inbound).
    Ack855,
    /// 856 ship notice / ASN (inbound).
    Asn856,
    /// 810 invoice (inbound).
    Invoice810,
    /// 832 price/sales catalog (inbound).
    Catalog832,
}

impl EdiDocType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Po850 => "850",
            Self::Ack855 => "855",
            Self::Asn856 => "856",
            Self::Invoice810 => "810",
            Self::Catalog832 => "832",
        }
    }
}

impl std::str::FromStr for EdiDocType {
    type Err = EdiError;

    fn from_str(value: &str) -> Result<Self, EdiError> {
        match value {
            "850" => Ok(Self::Po850),
            "855" => Ok(Self::Ack855),
            "856" => Ok(Self::Asn856),
            "810" => Ok(Self::Invoice810),
            "832" => Ok(Self::Catalog832),
            other => Err(EdiError::Parse(format!("unknown EDI doc_type {other:?}"))),
        }
    }
}

/// Direction of an EDI document relative to this system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdiDirection {
    Inbound,
    Outbound,
}

impl EdiDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbound => "in",
            Self::Outbound => "out",
        }
    }
}

impl std::str::FromStr for EdiDirection {
    type Err = EdiError;

    fn from_str(value: &str) -> Result<Self, EdiError> {
        match value {
            "in" => Ok(Self::Inbound),
            "out" => Ok(Self::Outbound),
            other => Err(EdiError::Parse(format!("unknown EDI direction {other:?}"))),
        }
    }
}

/// One row of `edi_documents`: the raw file text plus its parsed form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdiDocument {
    /// UUIDv7 id.
    pub id: Uuid,
    pub doc_type: EdiDocType,
    pub direction: EdiDirection,
    pub vendor_id: String,
    /// Linked PO, when the document relates to one (e.g. an 856/810
    /// answering an 850).
    pub related_po_id: Option<Uuid>,
    /// Raw document text, exactly as received/sent.
    pub raw_text: String,
    /// Parsed JSON form (e.g. a serialized `Shipment` / `Invoice`).
    pub parsed_json: Value,
    pub received_at: DateTime<Utc>,
}

fn uuid_str(id: Uuid) -> String {
    id.to_string()
}

fn opt_uuid_str(id: Option<Uuid>) -> Option<String> {
    id.map(|i| i.to_string())
}

fn dec_str(amount: Decimal) -> String {
    amount.to_string()
}

fn dt_str(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

fn text_col(row: &AnyRow, name: &str) -> Result<String, EdiError> {
    row.try_get(name)
        .map_err(|e| EdiError::Parse(format!("column {name}: {e}")))
}

fn opt_text_col(row: &AnyRow, name: &str) -> Result<Option<String>, EdiError> {
    row.try_get(name)
        .map_err(|e| EdiError::Parse(format!("column {name}: {e}")))
}

fn uuid_col(row: &AnyRow, name: &str) -> Result<Uuid, EdiError> {
    Uuid::parse_str(&text_col(row, name)?)
        .map_err(|e| EdiError::Parse(format!("column {name} is not a UUID: {e}")))
}

fn opt_uuid_col(row: &AnyRow, name: &str) -> Result<Option<Uuid>, EdiError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => Uuid::parse_str(&text)
            .map(Some)
            .map_err(|e| EdiError::Parse(format!("column {name} is not a UUID: {e}"))),
    }
}

fn dec_col(row: &AnyRow, name: &str) -> Result<Decimal, EdiError> {
    Decimal::from_str(&text_col(row, name)?)
        .map_err(|e| EdiError::Parse(format!("column {name} is not a decimal: {e}")))
}

fn dt_col(row: &AnyRow, name: &str) -> Result<DateTime<Utc>, EdiError> {
    let text = text_col(row, name)?;
    DateTime::parse_from_rfc3339(&text)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| EdiError::Parse(format!("column {name} is not RFC 3339: {e}")))
}

fn json_col(row: &AnyRow, name: &str) -> Result<Value, EdiError> {
    let text = text_col(row, name)?;
    serde_json::from_str(&text)
        .map_err(|e| EdiError::Parse(format!("column {name} is not JSON: {e}")))
}

fn i32_col(row: &AnyRow, name: &str) -> Result<i32, EdiError> {
    // SQLite reports INTEGER columns as i64; PostgreSQL as i32.
    if let Ok(value) = row.try_get::<i32, _>(name) {
        return Ok(value);
    }
    let value: i64 = row
        .try_get(name)
        .map_err(|e| EdiError::Parse(format!("column {name}: {e}")))?;
    i32::try_from(value).map_err(|e| EdiError::Parse(format!("column {name}: {e}")))
}

/// Insert a purchase order header plus all of its lines.
pub async fn insert_purchase_order(pool: &AnyPool, po: &PurchaseOrder) -> Result<(), EdiError> {
    sqlx::query(
        "INSERT INTO purchase_orders (id, po_number, vendor_id, store_id, status, total, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(uuid_str(po.id))
    .bind(&po.po_number)
    .bind(&po.vendor_id)
    .bind(uuid_str(po.store_id))
    .bind(po.status.as_str())
    .bind(dec_str(po.total()))
    .bind(dt_str(po.created_at))
    .execute(pool)
    .await?;
    for line in &po.lines {
        insert_purchase_order_line(pool, po.id, line).await?;
    }
    Ok(())
}

async fn insert_purchase_order_line(
    pool: &AnyPool,
    po_id: Uuid,
    line: &PurchaseOrderLine,
) -> Result<(), EdiError> {
    sqlx::query(
        "INSERT INTO purchase_order_lines (id, po_id, product_id, line_no, vendor_sku, description, qty, uom, unit_cost) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(uuid_str(Uuid::now_v7()))
    .bind(uuid_str(po_id))
    .bind(uuid_str(line.product_id))
    .bind(i32::try_from(line.line_no).unwrap_or(i32::MAX))
    .bind(&line.vendor_sku)
    .bind(&line.description)
    .bind(dec_str(line.qty))
    .bind(&line.uom)
    .bind(dec_str(line.unit_cost))
    .execute(pool)
    .await?;
    Ok(())
}

fn purchase_order_from_rows(
    header: &AnyRow,
    line_rows: &[AnyRow],
) -> Result<PurchaseOrder, EdiError> {
    let mut lines = Vec::new();
    for row in line_rows {
        lines.push(PurchaseOrderLine {
            line_no: i32_col(row, "line_no")? as u32,
            product_id: uuid_col(row, "product_id")?,
            vendor_sku: text_col(row, "vendor_sku")?,
            description: text_col(row, "description")?,
            qty: dec_col(row, "qty")?,
            uom: text_col(row, "uom")?,
            unit_cost: dec_col(row, "unit_cost")?,
        });
    }
    lines.sort_by_key(|line| line.line_no);
    Ok(PurchaseOrder {
        id: uuid_col(header, "id")?,
        po_number: text_col(header, "po_number")?,
        vendor_id: text_col(header, "vendor_id")?,
        store_id: uuid_col(header, "store_id")?,
        status: PoStatus::from_str(&text_col(header, "status")?)?,
        lines,
        created_at: dt_col(header, "created_at")?,
    })
}

/// Fetch a purchase order by id, with its lines in order.
pub async fn get_purchase_order(pool: &AnyPool, id: Uuid) -> Result<PurchaseOrder, EdiError> {
    let header = sqlx::query("SELECT * FROM purchase_orders WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => EdiError::Parse(format!("purchase order {id} not found")),
            other => EdiError::Sqlx(other),
        })?;
    let line_rows =
        sqlx::query("SELECT * FROM purchase_order_lines WHERE po_id = $1 ORDER BY line_no ASC")
            .bind(uuid_str(id))
            .fetch_all(pool)
            .await?;
    purchase_order_from_rows(&header, &line_rows)
}

/// List purchase orders for a vendor, newest first.
pub async fn list_purchase_orders_for_vendor(
    pool: &AnyPool,
    vendor_id: &str,
) -> Result<Vec<PurchaseOrder>, EdiError> {
    let headers =
        sqlx::query("SELECT * FROM purchase_orders WHERE vendor_id = $1 ORDER BY created_at DESC")
            .bind(vendor_id)
            .fetch_all(pool)
            .await?;
    let mut out = Vec::new();
    for header in &headers {
        let id = uuid_col(header, "id")?;
        let line_rows =
            sqlx::query("SELECT * FROM purchase_order_lines WHERE po_id = $1 ORDER BY line_no ASC")
                .bind(uuid_str(id))
                .fetch_all(pool)
                .await?;
        out.push(purchase_order_from_rows(header, &line_rows)?);
    }
    Ok(out)
}

/// Insert one EDI document row (raw text + parsed JSON).
pub async fn insert_edi_document(pool: &AnyPool, doc: &EdiDocument) -> Result<(), EdiError> {
    sqlx::query(
        "INSERT INTO edi_documents (id, doc_type, direction, vendor_id, related_po_id, raw_text, parsed_json, received_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(uuid_str(doc.id))
    .bind(doc.doc_type.as_str())
    .bind(doc.direction.as_str())
    .bind(&doc.vendor_id)
    .bind(opt_uuid_str(doc.related_po_id))
    .bind(&doc.raw_text)
    .bind(doc.parsed_json.to_string())
    .bind(dt_str(doc.received_at))
    .execute(pool)
    .await?;
    Ok(())
}

fn edi_document_from_row(row: &AnyRow) -> Result<EdiDocument, EdiError> {
    Ok(EdiDocument {
        id: uuid_col(row, "id")?,
        doc_type: EdiDocType::from_str(&text_col(row, "doc_type")?)?,
        direction: EdiDirection::from_str(&text_col(row, "direction")?)?,
        vendor_id: text_col(row, "vendor_id")?,
        related_po_id: opt_uuid_col(row, "related_po_id")?,
        raw_text: text_col(row, "raw_text")?,
        parsed_json: json_col(row, "parsed_json")?,
        received_at: dt_col(row, "received_at")?,
    })
}

/// Fetch one EDI document by id.
pub async fn get_edi_document(pool: &AnyPool, id: Uuid) -> Result<EdiDocument, EdiError> {
    let row = sqlx::query("SELECT * FROM edi_documents WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => EdiError::Parse(format!("EDI document {id} not found")),
            other => EdiError::Sqlx(other),
        })?;
    edi_document_from_row(&row)
}

/// List EDI documents linked to a purchase order, newest first.
pub async fn list_edi_documents_for_po(
    pool: &AnyPool,
    po_id: Uuid,
) -> Result<Vec<EdiDocument>, EdiError> {
    let rows = sqlx::query(
        "SELECT * FROM edi_documents WHERE related_po_id = $1 ORDER BY received_at DESC",
    )
    .bind(uuid_str(po_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(edi_document_from_row).collect()
}

// --- Phase 4 (v0.5) repositories (`0009_phase4_edi`) -----------------------

fn opt_dec_col(row: &AnyRow, name: &str) -> Result<Option<Decimal>, EdiError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => Decimal::from_str(&text)
            .map(Some)
            .map_err(|e| EdiError::Parse(format!("column {name} is not a decimal: {e}"))),
    }
}

fn opt_date_col(row: &AnyRow, name: &str) -> Result<Option<NaiveDate>, EdiError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => NaiveDate::parse_from_str(&text, "%Y-%m-%d")
            .map(Some)
            .map_err(|e| EdiError::Parse(format!("column {name} is not a date: {e}"))),
    }
}

fn date_col(row: &AnyRow, name: &str) -> Result<NaiveDate, EdiError> {
    NaiveDate::parse_from_str(&text_col(row, name)?, "%Y-%m-%d")
        .map_err(|e| EdiError::Parse(format!("column {name} is not a date: {e}")))
}

fn opt_dt_col(row: &AnyRow, name: &str) -> Result<Option<DateTime<Utc>>, EdiError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => DateTime::parse_from_rfc3339(&text)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|e| EdiError::Parse(format!("column {name} is not RFC 3339: {e}"))),
    }
}

fn opt_i32_col(row: &AnyRow, name: &str) -> Result<Option<i32>, EdiError> {
    if let Ok(value) = row.try_get::<Option<i32>, _>(name) {
        return Ok(value);
    }
    let value: Option<i64> = row
        .try_get(name)
        .map_err(|e| EdiError::Parse(format!("column {name}: {e}")))?;
    value
        .map(i32::try_from)
        .transpose()
        .map_err(|e| EdiError::Parse(format!("column {name}: {e}")))
}

fn opt_date_str(date: Option<NaiveDate>) -> Option<String> {
    date.map(|d| d.to_string())
}

fn opt_dec_str(amount: Option<Decimal>) -> Option<String> {
    amount.map(|a| a.to_string())
}

/// One stored 855 acknowledgment: its row id, the PO it answers,
/// and the parsed acknowledgment. `vendor_id` is not stored (the
/// vendor comes from the linked PO) and reads back as `None`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PoAcknowledgmentRow {
    pub id: Uuid,
    pub po_id: Uuid,
    pub ack: PoAcknowledgment,
    pub received_at: DateTime<Utc>,
}

/// Insert a parsed 855 acknowledgment plus its lines. Returns the
/// new acknowledgment id.
pub async fn insert_po_acknowledgment(
    pool: &AnyPool,
    po_id: Uuid,
    ack: &PoAcknowledgment,
) -> Result<Uuid, EdiError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO po_acknowledgments (id, po_id, po_number, ack_date, received_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid_str(id))
    .bind(uuid_str(po_id))
    .bind(&ack.po_number)
    .bind(opt_date_str(ack.ack_date))
    .bind(dt_str(Utc::now()))
    .execute(pool)
    .await?;
    for line in &ack.lines {
        sqlx::query(
            "INSERT INTO po_acknowledgment_lines (id, ack_id, line_no, vendor_sku, ack_code, qty_acknowledged, uom, unit_cost, promised_date) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(uuid_str(Uuid::now_v7()))
        .bind(uuid_str(id))
        .bind(i32::try_from(line.line_no).unwrap_or(i32::MAX))
        .bind(&line.vendor_sku)
        .bind(line.ack_code.as_x12())
        .bind(dec_str(line.qty_acknowledged))
        .bind(&line.uom)
        .bind(opt_dec_str(line.unit_cost))
        .bind(opt_date_str(line.promised_date))
        .execute(pool)
        .await?;
    }
    Ok(id)
}

async fn po_acknowledgment_lines(pool: &AnyPool, ack_id: Uuid) -> Result<Vec<AckLine>, EdiError> {
    let rows =
        sqlx::query("SELECT * FROM po_acknowledgment_lines WHERE ack_id = $1 ORDER BY line_no ASC")
            .bind(uuid_str(ack_id))
            .fetch_all(pool)
            .await?;
    let mut lines = Vec::new();
    for row in &rows {
        lines.push(AckLine {
            line_no: i32_col(row, "line_no")? as u32,
            vendor_sku: text_col(row, "vendor_sku")?,
            ack_code: AckCode::from_x12(&text_col(row, "ack_code")?)?,
            qty_acknowledged: dec_col(row, "qty_acknowledged")?,
            uom: text_col(row, "uom")?,
            unit_cost: opt_dec_col(row, "unit_cost")?,
            promised_date: opt_date_col(row, "promised_date")?,
        });
    }
    Ok(lines)
}

fn po_acknowledgment_from_row(
    header: &AnyRow,
    lines: Vec<AckLine>,
) -> Result<PoAcknowledgmentRow, EdiError> {
    Ok(PoAcknowledgmentRow {
        id: uuid_col(header, "id")?,
        po_id: uuid_col(header, "po_id")?,
        ack: PoAcknowledgment {
            po_number: text_col(header, "po_number")?,
            ack_date: opt_date_col(header, "ack_date")?,
            vendor_id: None,
            lines,
        },
        received_at: dt_col(header, "received_at")?,
    })
}

/// Fetch one stored 855 acknowledgment by id, with its lines.
pub async fn get_po_acknowledgment(
    pool: &AnyPool,
    id: Uuid,
) -> Result<PoAcknowledgmentRow, EdiError> {
    let header = sqlx::query("SELECT * FROM po_acknowledgments WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                EdiError::Parse(format!("PO acknowledgment {id} not found"))
            }
            other => EdiError::Sqlx(other),
        })?;
    let lines = po_acknowledgment_lines(pool, id).await?;
    po_acknowledgment_from_row(&header, lines)
}

/// List stored 855 acknowledgments for a PO, newest first.
pub async fn list_po_acknowledgments_for_po(
    pool: &AnyPool,
    po_id: Uuid,
) -> Result<Vec<PoAcknowledgmentRow>, EdiError> {
    let headers =
        sqlx::query("SELECT * FROM po_acknowledgments WHERE po_id = $1 ORDER BY received_at DESC")
            .bind(uuid_str(po_id))
            .fetch_all(pool)
            .await?;
    let mut out = Vec::new();
    for header in &headers {
        let id = uuid_col(header, "id")?;
        let lines = po_acknowledgment_lines(pool, id).await?;
        out.push(po_acknowledgment_from_row(header, lines)?);
    }
    Ok(out)
}

/// One stored three-way match report: its row id, the PO it
/// matches, and the report itself.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InvoiceMatchReportRow {
    pub id: Uuid,
    pub po_id: Uuid,
    pub report: InvoiceMatchReport,
}

/// Insert a three-way match report plus its lines.
pub async fn insert_invoice_match_report(
    pool: &AnyPool,
    po_id: Uuid,
    report: &InvoiceMatchReport,
) -> Result<(), EdiError> {
    sqlx::query(
        "INSERT INTO invoice_match_reports (id, po_id, po_number, invoice_number, shipment_id, status, po_total, invoiced_total, total_variance, stated_total_difference, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(uuid_str(report.id))
    .bind(uuid_str(po_id))
    .bind(&report.po_number)
    .bind(&report.invoice_number)
    .bind(&report.shipment_id)
    .bind(report.status.as_str())
    .bind(dec_str(report.po_total))
    .bind(dec_str(report.invoiced_total))
    .bind(dec_str(report.total_variance_amount))
    .bind(opt_dec_str(report.stated_total_difference))
    .bind(dt_str(report.created_at))
    .execute(pool)
    .await?;
    for line in &report.lines {
        sqlx::query(
            "INSERT INTO invoice_match_lines (id, match_id, po_line_no, vendor_sku, po_qty, received_qty, invoiced_qty, po_unit_cost, invoiced_unit_price, qty_variance, price_variance_per_unit, qty_variance_amount, price_variance_amount, total_variance_amount, status, variance_kind) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(uuid_str(Uuid::now_v7()))
        .bind(uuid_str(report.id))
        .bind(line.po_line_no.map(|n| i32::try_from(n).unwrap_or(i32::MAX)))
        .bind(&line.vendor_sku)
        .bind(dec_str(line.po_qty))
        .bind(dec_str(line.received_qty))
        .bind(dec_str(line.invoiced_qty))
        .bind(dec_str(line.po_unit_cost))
        .bind(dec_str(line.invoiced_unit_price))
        .bind(dec_str(line.qty_variance))
        .bind(dec_str(line.price_variance_per_unit))
        .bind(dec_str(line.qty_variance_amount))
        .bind(dec_str(line.price_variance_amount))
        .bind(dec_str(line.total_variance_amount))
        .bind(line.status.as_str())
        .bind(line.variance_kind.as_str())
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn invoice_match_lines(pool: &AnyPool, match_id: Uuid) -> Result<Vec<MatchLine>, EdiError> {
    // Lines carry no sequence column; order by SKU for a stable
    // read-back (callers match lines by SKU, not position).
    let rows = sqlx::query(
        "SELECT * FROM invoice_match_lines WHERE match_id = $1 ORDER BY vendor_sku ASC",
    )
    .bind(uuid_str(match_id))
    .fetch_all(pool)
    .await?;
    let mut lines = Vec::new();
    for row in &rows {
        lines.push(MatchLine {
            po_line_no: opt_i32_col(row, "po_line_no")?.map(|n| n as u32),
            vendor_sku: text_col(row, "vendor_sku")?,
            po_qty: dec_col(row, "po_qty")?,
            received_qty: dec_col(row, "received_qty")?,
            invoiced_qty: dec_col(row, "invoiced_qty")?,
            po_unit_cost: dec_col(row, "po_unit_cost")?,
            invoiced_unit_price: dec_col(row, "invoiced_unit_price")?,
            qty_variance: dec_col(row, "qty_variance")?,
            price_variance_per_unit: dec_col(row, "price_variance_per_unit")?,
            qty_variance_amount: dec_col(row, "qty_variance_amount")?,
            price_variance_amount: dec_col(row, "price_variance_amount")?,
            total_variance_amount: dec_col(row, "total_variance_amount")?,
            status: MatchStatus::from_str(&text_col(row, "status")?)?,
            variance_kind: VarianceKind::from_str(&text_col(row, "variance_kind")?)?,
        });
    }
    Ok(lines)
}

fn invoice_match_report_from_row(
    header: &AnyRow,
    lines: Vec<MatchLine>,
) -> Result<InvoiceMatchReportRow, EdiError> {
    Ok(InvoiceMatchReportRow {
        id: uuid_col(header, "id")?,
        po_id: uuid_col(header, "po_id")?,
        report: InvoiceMatchReport {
            id: uuid_col(header, "id")?,
            po_number: text_col(header, "po_number")?,
            invoice_number: text_col(header, "invoice_number")?,
            shipment_id: text_col(header, "shipment_id")?,
            status: MatchStatus::from_str(&text_col(header, "status")?)?,
            lines,
            po_total: dec_col(header, "po_total")?,
            invoiced_total: dec_col(header, "invoiced_total")?,
            total_variance_amount: dec_col(header, "total_variance")?,
            stated_total_difference: opt_dec_col(header, "stated_total_difference")?,
            created_at: dt_col(header, "created_at")?,
        },
    })
}

/// Fetch one stored match report by id, with its lines.
pub async fn get_invoice_match_report(
    pool: &AnyPool,
    id: Uuid,
) -> Result<InvoiceMatchReportRow, EdiError> {
    let header = sqlx::query("SELECT * FROM invoice_match_reports WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                EdiError::Parse(format!("invoice match report {id} not found"))
            }
            other => EdiError::Sqlx(other),
        })?;
    let lines = invoice_match_lines(pool, id).await?;
    invoice_match_report_from_row(&header, lines)
}

/// List stored match reports for a PO, newest first.
pub async fn list_invoice_match_reports_for_po(
    pool: &AnyPool,
    po_id: Uuid,
) -> Result<Vec<InvoiceMatchReportRow>, EdiError> {
    let headers = sqlx::query(
        "SELECT * FROM invoice_match_reports WHERE po_id = $1 ORDER BY created_at DESC",
    )
    .bind(uuid_str(po_id))
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for header in &headers {
        let id = uuid_col(header, "id")?;
        let lines = invoice_match_lines(pool, id).await?;
        out.push(invoice_match_report_from_row(header, lines)?);
    }
    Ok(out)
}

/// One stored 832 catalog application: the application plus the
/// time it was marked applied, when it was.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PriceCatalogApplicationRow {
    pub id: Uuid,
    pub application: PriceCatalogApplication,
    pub applied_at: Option<DateTime<Utc>>,
}

/// Insert a price-catalog application plus its lines. `applied_at`
/// is `Some` only when the application was already reviewed and
/// marked applied.
pub async fn insert_price_catalog_application(
    pool: &AnyPool,
    application: &PriceCatalogApplication,
    applied_at: Option<DateTime<Utc>>,
) -> Result<(), EdiError> {
    sqlx::query(
        "INSERT INTO price_catalog_applications (id, vendor_id, catalog_id, catalog_date, status, reviewed_by, applied_at, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(uuid_str(application.id))
    .bind(&application.vendor_id)
    .bind(&application.catalog_id)
    .bind(application.catalog_date.to_string())
    .bind(application.status.as_str())
    .bind(application.reviewed_by.clone())
    .bind(applied_at.map(dt_str))
    .bind(dt_str(Utc::now()))
    .execute(pool)
    .await?;
    for line in &application.lines {
        sqlx::query(
            "INSERT INTO price_catalog_lines (id, application_id, vendor_sku, action, old_cost, old_list_price, new_cost, new_list_price, result, note) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(uuid_str(Uuid::now_v7()))
        .bind(uuid_str(application.id))
        .bind(&line.vendor_sku)
        .bind(line.action.as_str())
        .bind(opt_dec_str(line.old_cost))
        .bind(opt_dec_str(line.old_list_price))
        .bind(opt_dec_str(line.new_cost))
        .bind(opt_dec_str(line.new_list_price))
        .bind(line.result.as_str())
        .bind(&line.note)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn price_catalog_lines(
    pool: &AnyPool,
    application_id: Uuid,
) -> Result<Vec<AppliedPriceLine>, EdiError> {
    // Lines carry no sequence column; order by SKU for a stable
    // read-back (callers match lines by SKU, not position).
    let rows = sqlx::query(
        "SELECT * FROM price_catalog_lines WHERE application_id = $1 ORDER BY vendor_sku ASC",
    )
    .bind(uuid_str(application_id))
    .fetch_all(pool)
    .await?;
    let mut lines = Vec::new();
    for row in &rows {
        lines.push(AppliedPriceLine {
            vendor_sku: text_col(row, "vendor_sku")?,
            action: CatalogAction::from_str(&text_col(row, "action")?)?,
            old_cost: opt_dec_col(row, "old_cost")?,
            old_list_price: opt_dec_col(row, "old_list_price")?,
            new_cost: opt_dec_col(row, "new_cost")?,
            new_list_price: opt_dec_col(row, "new_list_price")?,
            result: PriceApplyResult::from_str(&text_col(row, "result")?)?,
            note: text_col(row, "note")?,
        });
    }
    Ok(lines)
}

fn price_catalog_application_from_row(
    header: &AnyRow,
    lines: Vec<AppliedPriceLine>,
) -> Result<PriceCatalogApplicationRow, EdiError> {
    Ok(PriceCatalogApplicationRow {
        id: uuid_col(header, "id")?,
        application: PriceCatalogApplication {
            id: uuid_col(header, "id")?,
            vendor_id: text_col(header, "vendor_id")?,
            catalog_id: text_col(header, "catalog_id")?,
            catalog_date: date_col(header, "catalog_date")?,
            status: ApplicationStatus::from_str(&text_col(header, "status")?)?,
            reviewed_by: opt_text_col(header, "reviewed_by")?,
            lines,
        },
        applied_at: opt_dt_col(header, "applied_at")?,
    })
}

/// Fetch one stored catalog application by id, with its lines.
pub async fn get_price_catalog_application(
    pool: &AnyPool,
    id: Uuid,
) -> Result<PriceCatalogApplicationRow, EdiError> {
    let header = sqlx::query("SELECT * FROM price_catalog_applications WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                EdiError::Parse(format!("price catalog application {id} not found"))
            }
            other => EdiError::Sqlx(other),
        })?;
    let lines = price_catalog_lines(pool, id).await?;
    price_catalog_application_from_row(&header, lines)
}

/// One EDI transport record: a file sent or received through a
/// vendor transport, for the audit trail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransportRecord {
    /// UUIDv7 id.
    pub id: Uuid,
    pub vendor_id: String,
    pub direction: EdiDirection,
    /// e.g. `local-sftp-stand-in`, `as2-file`.
    pub transport: String,
    pub file_name: String,
    pub doc_type: Option<EdiDocType>,
    /// `sent`, `received`, `archived`, `failed`.
    pub status: String,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Insert one transport record.
pub async fn insert_transport_record(
    pool: &AnyPool,
    record: &TransportRecord,
) -> Result<(), EdiError> {
    sqlx::query(
        "INSERT INTO edi_transport_records (id, vendor_id, direction, transport, file_name, doc_type, status, error, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(uuid_str(record.id))
    .bind(&record.vendor_id)
    .bind(record.direction.as_str())
    .bind(&record.transport)
    .bind(&record.file_name)
    .bind(record.doc_type.map(EdiDocType::as_str))
    .bind(&record.status)
    .bind(record.error.clone())
    .bind(dt_str(record.created_at))
    .execute(pool)
    .await?;
    Ok(())
}

fn transport_record_from_row(row: &AnyRow) -> Result<TransportRecord, EdiError> {
    Ok(TransportRecord {
        id: uuid_col(row, "id")?,
        vendor_id: text_col(row, "vendor_id")?,
        direction: EdiDirection::from_str(&text_col(row, "direction")?)?,
        transport: text_col(row, "transport")?,
        file_name: text_col(row, "file_name")?,
        doc_type: match opt_text_col(row, "doc_type")? {
            None => None,
            Some(text) => Some(EdiDocType::from_str(&text)?),
        },
        status: text_col(row, "status")?,
        error: opt_text_col(row, "error")?,
        created_at: dt_col(row, "created_at")?,
    })
}

/// List transport records for a vendor, newest first.
pub async fn list_transport_records_for_vendor(
    pool: &AnyPool,
    vendor_id: &str,
) -> Result<Vec<TransportRecord>, EdiError> {
    let rows = sqlx::query(
        "SELECT * FROM edi_transport_records WHERE vendor_id = $1 ORDER BY created_at DESC",
    )
    .bind(vendor_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(transport_record_from_row).collect()
}
