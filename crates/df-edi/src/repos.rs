//! Repositories for the Phase 2 EDI tables (`0004_phase2_edi`).
//!
//! These live in `df-edi`, not in `df-db`: they operate on the
//! `df-db`-owned [`df_db::AnyPool`] type and follow the `df-db`
//! conventions — `?` placeholders (rewritten to `$N` for PostgreSQL),
//! ids / money / timestamps crossing the driver as text (UUID as
//! hyphenated text, money as an exact decimal string, timestamps as
//! RFC 3339). No `query!` macros, so the workspace builds with
//! `SQLX_OFFLINE=true` and no live database.
//!
//! PostgreSQL (HQ) enforces the FK graph; SQLite (edge) keeps the
//! edge-owned chains (`purchase_order_lines` / `edi_documents` →
//! `purchase_orders`) and omits FKs to HQ reference data.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::any::AnyRow;
use sqlx::{AnyPool, Row};
use uuid::Uuid;

use crate::error::EdiError;
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
        "INSERT INTO purchase_orders (id, po_number, vendor_id, store_id, status, total, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
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
        "INSERT INTO purchase_order_lines (id, po_id, product_id, line_no, vendor_sku, description, qty, uom, unit_cost) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    let header = sqlx::query("SELECT * FROM purchase_orders WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => EdiError::Parse(format!("purchase order {id} not found")),
            other => EdiError::Sqlx(other),
        })?;
    let line_rows =
        sqlx::query("SELECT * FROM purchase_order_lines WHERE po_id = ? ORDER BY line_no ASC")
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
        sqlx::query("SELECT * FROM purchase_orders WHERE vendor_id = ? ORDER BY created_at DESC")
            .bind(vendor_id)
            .fetch_all(pool)
            .await?;
    let mut out = Vec::new();
    for header in &headers {
        let id = uuid_col(header, "id")?;
        let line_rows =
            sqlx::query("SELECT * FROM purchase_order_lines WHERE po_id = ? ORDER BY line_no ASC")
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
        "INSERT INTO edi_documents (id, doc_type, direction, vendor_id, related_po_id, raw_text, parsed_json, received_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
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
    let row = sqlx::query("SELECT * FROM edi_documents WHERE id = ?")
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
        "SELECT * FROM edi_documents WHERE related_po_id = ? ORDER BY received_at DESC",
    )
    .bind(uuid_str(po_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(edi_document_from_row).collect()
}
