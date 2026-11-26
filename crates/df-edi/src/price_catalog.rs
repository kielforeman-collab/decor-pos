//! X12 832 price/sales catalogs (Phase 4, v0.5).
//!
//! A vendor sends an 832 when prices change or products are added
//! or dropped. This module builds and parses 832 documents and
//! turns one into a **reviewed apply report** — it never writes to
//! the live product catalog by itself.
//!
//! ## Segment profile used by this crate
//!
//! ```text
//! ST*832*<control>~
//! BCT*00*<catalog id>*<catalog date CCYYMMDD>~
//! LIN*<line>*VP*<vendor sku>~
//! PID*F****<description>~
//! CTP*LIST*<list price>~                 (when a list price is given)
//! CTP*COST*<vendor cost>~                (when a cost is given)
//! REF*UOM*<unit of measure>~
//! REF*ACTION*ADD|CHG|DROP~               (one per item)
//! DTM*007*<effective date CCYYMMDD>~     (when an effective date is given)
//! SE*...~
//! ```
//!
//! `CTP01` here is the crate's price-type tag (`LIST` / `COST`)
//! and `CTP02` the amount — a deliberately narrow profile,
//! documented so a vendor pack can map its own `CTP` forms onto
//! it. `REF*ACTION` carries the item action because an 832 `LIN`
//! has no add/change/drop field of its own.
//!
//! ## Applying a catalog
//!
//! [`build_price_apply_report`] compares the catalog against the
//! current prices the caller supplies (`&[CurrentPrice]`) and
//! returns a [`PriceCatalogApplication`] with status
//! `pending_review` and one line per catalog item: old vs new cost
//! and list price, and the result (`applied` is only a *proposed*
//! outcome — nothing is written). A human (or a later, wired-up
//! endpoint) marks the report [`PriceCatalogApplication::mark_applied`]
//! after review. Drops of SKUs the store does not carry, and
//! changes that equal the current price, are reported as skipped
//! with a note instead of silently "applying".

use std::str::FromStr;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{EdiError, Result};
use crate::x12::{
    Segment, envelope_close, envelope_open, parse_document, parse_x12_date, render_document,
    require, wrap_transaction, x12_date,
};

/// What a catalog item does to the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogAction {
    /// New product.
    Add,
    /// Price (or description) change to an existing product.
    Change,
    /// Product discontinued by the vendor.
    Drop,
}

impl CatalogAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "ADD",
            Self::Change => "CHG",
            Self::Drop => "DROP",
        }
    }
}

impl std::str::FromStr for CatalogAction {
    type Err = EdiError;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim() {
            "ADD" => Ok(Self::Add),
            "CHG" => Ok(Self::Change),
            "DROP" => Ok(Self::Drop),
            other => Err(EdiError::Parse(format!(
                "unknown catalog action {other:?} (expected ADD, CHG or DROP)"
            ))),
        }
    }
}

/// One item on a vendor price catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceCatalogItem {
    pub vendor_sku: String,
    pub description: String,
    /// Vendor cost, when the catalog gives one.
    pub cost: Option<Decimal>,
    /// List price, when the catalog gives one.
    pub list_price: Option<Decimal>,
    pub uom: String,
    pub action: CatalogAction,
    /// Date the new price takes effect, when given.
    pub effective_date: Option<NaiveDate>,
}

/// A vendor price/sales catalog (X12 832).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceCatalog {
    pub vendor_id: String,
    pub catalog_id: String,
    pub catalog_date: NaiveDate,
    pub items: Vec<PriceCatalogItem>,
}

/// Serialize a price catalog as an X12 832.
///
/// Envelope: `ISA/GS/ST/BCT/LIN…/SE/GE/IEA`, functional group `SC`,
/// real control numbers and a correct `SE` count.
pub fn price_catalog_to_x12_832(
    catalog: &PriceCatalog,
    sender_id: &str,
    receiver_id: &str,
    control: u32,
) -> String {
    let time = chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap();
    let mut segments = envelope_open(
        "SC",
        sender_id,
        receiver_id,
        catalog.catalog_date,
        time,
        control,
    );
    let mut body = vec![Segment::new(
        "BCT",
        &["00", &catalog.catalog_id, &x12_date(catalog.catalog_date)],
    )];
    for (index, item) in catalog.items.iter().enumerate() {
        let line_no = index + 1;
        body.push(Segment::new(
            "LIN",
            &[&line_no.to_string(), "VP", &item.vendor_sku],
        ));
        body.push(Segment::new("PID", &["F", "", "", "", &item.description]));
        if let Some(list_price) = item.list_price {
            body.push(Segment::new("CTP", &["LIST", &list_price.to_string()]));
        }
        if let Some(cost) = item.cost {
            body.push(Segment::new("CTP", &["COST", &cost.to_string()]));
        }
        body.push(Segment::new("REF", &["UOM", &item.uom]));
        body.push(Segment::new("REF", &["ACTION", item.action.as_str()]));
        if let Some(effective) = item.effective_date {
            body.push(Segment::new("DTM", &["007", &x12_date(effective)]));
        }
    }
    segments.extend(wrap_transaction("832", control, &body));
    segments.extend(envelope_close(control, control));
    render_document(&segments)
}

fn parse_decimal_field(value: &str, what: &str) -> Result<Decimal> {
    Decimal::from_str(value.trim())
        .map_err(|e| EdiError::InvalidDecimal(format!("{what}: {value:?}"), e))
}

/// Parse an X12 832 document back into a [`PriceCatalog`].
///
/// Requires `ST*832` and a `BCT` with a catalog id and date; every
/// `LIN` starts an item, and a `LIN` with no `REF*ACTION` is an
/// error (the action is what makes an item add/change/drop). A
/// `CTP`, `PID`, `REF` or `DTM` with no preceding `LIN` is an error.
pub fn parse_832(text: &str) -> Result<PriceCatalog> {
    let segments = parse_document(text)?;
    let st = require(&segments, "ST")?;
    if st.get(1) != Some("832") {
        return Err(EdiError::Parse(format!(
            "expected transaction set 832, found {:?}",
            st.get(1)
        )));
    }
    let bct = require(&segments, "BCT")?;
    let catalog_id = bct
        .get_trimmed(2)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| EdiError::Parse("BCT02 catalog id is missing".to_string()))?
        .to_string();
    let catalog_date = match bct.get_trimmed(3) {
        Some(value) if !value.is_empty() => parse_x12_date(value)?,
        _ => {
            return Err(EdiError::Parse("BCT03 catalog date is missing".to_string()));
        }
    };
    let vendor_id = segments
        .iter()
        .find(|s| s.tag == "GS")
        .and_then(|gs| gs.get_trimmed(2))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_default();

    let mut items: Vec<PriceCatalogItem> = Vec::new();
    let mut pending: Option<PriceCatalogItem> = None;
    let mut pending_action: Option<CatalogAction> = None;
    let flush = |pending: &mut Option<PriceCatalogItem>,
                 action: &mut Option<CatalogAction>,
                 items: &mut Vec<PriceCatalogItem>|
     -> Result<()> {
        if let Some(item) = pending.take() {
            let Some(action) = action.take() else {
                return Err(EdiError::Parse(format!(
                    "832 item {:?} has no REF*ACTION (ADD, CHG or DROP)",
                    item.vendor_sku
                )));
            };
            let mut item = item;
            item.action = action;
            items.push(item);
        }
        Ok(())
    };
    for segment in &segments {
        match segment.tag.as_str() {
            "LIN" => {
                flush(&mut pending, &mut pending_action, &mut items)?;
                let vendor_sku = segment.get_trimmed(3).unwrap_or("").to_string();
                pending = Some(PriceCatalogItem {
                    vendor_sku,
                    description: String::new(),
                    cost: None,
                    list_price: None,
                    uom: "EA".to_string(),
                    action: CatalogAction::Change,
                    effective_date: None,
                });
            }
            "PID" => {
                let Some(mut item) = pending.take() else {
                    return Err(EdiError::Parse(
                        "PID description with no preceding LIN segment".to_string(),
                    ));
                };
                item.description = segment.get_trimmed(5).unwrap_or("").to_string();
                pending = Some(item);
            }
            "CTP" => {
                let Some(mut item) = pending.take() else {
                    return Err(EdiError::Parse(
                        "CTP price with no preceding LIN segment".to_string(),
                    ));
                };
                let amount = parse_decimal_field(segment.get(2).unwrap_or(""), "CTP price")?;
                match segment.get_trimmed(1) {
                    Some("LIST") => item.list_price = Some(amount),
                    Some("COST") => item.cost = Some(amount),
                    other => {
                        return Err(EdiError::Parse(format!(
                            "unknown CTP price type {other:?} (expected LIST or COST)"
                        )));
                    }
                }
                pending = Some(item);
            }
            "REF" => {
                if pending.is_none() {
                    return Err(EdiError::Parse(
                        "REF with no preceding LIN segment".to_string(),
                    ));
                }
                match segment.get_trimmed(1) {
                    Some("ACTION") => {
                        pending_action = Some(CatalogAction::from_str(
                            segment.get_trimmed(2).unwrap_or(""),
                        )?);
                    }
                    Some("UOM") => {
                        let mut item = pending.take().unwrap();
                        item.uom = segment.get_trimmed(2).unwrap_or("").to_string();
                        pending = Some(item);
                    }
                    _ => {}
                }
            }
            "DTM" => {
                if segment.get_trimmed(1) != Some("007") {
                    continue;
                }
                let Some(mut item) = pending.take() else {
                    return Err(EdiError::Parse(
                        "DTM effective date with no preceding LIN segment".to_string(),
                    ));
                };
                item.effective_date = Some(parse_x12_date(segment.get_trimmed(2).unwrap_or(""))?);
                pending = Some(item);
            }
            _ => {}
        }
    }
    flush(&mut pending, &mut pending_action, &mut items)?;
    if items.is_empty() {
        return Err(EdiError::MissingSegment("LIN"));
    }
    Ok(PriceCatalog {
        vendor_id,
        catalog_id,
        catalog_date,
        items,
    })
}

// --- Applying a catalog (reviewed, never silent) ---------------------------

/// A product's current prices, as supplied by the caller when a
/// catalog is applied. The apply step reads these; it does not
/// write to the catalog itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrentPrice {
    pub vendor_sku: String,
    pub cost: Decimal,
    pub list_price: Decimal,
}

/// What the apply report proposes for one catalog item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PriceApplyResult {
    /// Price(s) differ from current — proposed to apply on review.
    Apply,
    /// New product — proposed to add on review.
    Add,
    /// Product dropped — proposed to deactivate on review.
    Drop,
    /// No change (same prices as current), or a drop of a SKU the
    /// store does not carry: nothing to do, reported with a note.
    Skipped,
}

impl PriceApplyResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Add => "add",
            Self::Drop => "drop",
            Self::Skipped => "skipped",
        }
    }
}

impl std::str::FromStr for PriceApplyResult {
    type Err = EdiError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "apply" => Ok(Self::Apply),
            "add" => Ok(Self::Add),
            "drop" => Ok(Self::Drop),
            "skipped" => Ok(Self::Skipped),
            other => Err(EdiError::Parse(format!(
                "unknown price apply result {other:?}"
            ))),
        }
    }
}

/// One line of a price-catalog apply report: before/after.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedPriceLine {
    pub vendor_sku: String,
    pub action: CatalogAction,
    pub old_cost: Option<Decimal>,
    pub old_list_price: Option<Decimal>,
    pub new_cost: Option<Decimal>,
    pub new_list_price: Option<Decimal>,
    pub result: PriceApplyResult,
    pub note: String,
}

/// Lifecycle of a catalog application: built as `pending_review`,
/// marked `applied` only after a human reviews the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApplicationStatus {
    PendingReview,
    Applied,
}

impl ApplicationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingReview => "pending_review",
            Self::Applied => "applied",
        }
    }
}

impl std::str::FromStr for ApplicationStatus {
    type Err = EdiError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "pending_review" => Ok(Self::PendingReview),
            "applied" => Ok(Self::Applied),
            other => Err(EdiError::Parse(format!(
                "unknown catalog application status {other:?}"
            ))),
        }
    }
}

/// A reviewed price-catalog application: the catalog's items with
/// before/after prices. This struct is the report — building it
/// changes no live prices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceCatalogApplication {
    /// UUIDv7 id.
    pub id: Uuid,
    pub vendor_id: String,
    pub catalog_id: String,
    pub catalog_date: NaiveDate,
    pub status: ApplicationStatus,
    /// Who reviewed and applied it, once [`Self::mark_applied`] runs.
    pub reviewed_by: Option<String>,
    pub lines: Vec<AppliedPriceLine>,
}

/// Build the apply report for a catalog against the store's current
/// prices. Rules, per item:
///
/// * `Add` for an unknown SKU → `Add`; for a known SKU → `Apply`
///   when a price differs, `Skipped` when identical.
/// * `Change` for a known SKU → `Apply` when a price differs,
///   `Skipped` ("already at catalog price") when identical; for an
///   unknown SKU → `Skipped` ("not carried — nothing to change").
/// * `Drop` for a known SKU → `Drop`; for an unknown SKU →
///   `Skipped` ("not carried").
/// * A price the catalog does not give keeps the current value in
///   the `new_*` column (it is not a change).
pub fn build_price_apply_report(
    catalog: &PriceCatalog,
    current: &[CurrentPrice],
) -> PriceCatalogApplication {
    let mut lines = Vec::new();
    for item in &catalog.items {
        let current_price = current
            .iter()
            .find(|price| price.vendor_sku == item.vendor_sku);
        let (result, note) = match (item.action, current_price) {
            (CatalogAction::Add, None) => (
                PriceApplyResult::Add,
                "new product — proposed to add".to_string(),
            ),
            (CatalogAction::Add, Some(_)) => {
                let differs = price_differs(item, current_price);
                if differs {
                    (
                        PriceApplyResult::Apply,
                        "already carried — proposed price update".to_string(),
                    )
                } else {
                    (
                        PriceApplyResult::Skipped,
                        "already carried at catalog price".to_string(),
                    )
                }
            }
            (CatalogAction::Change, Some(_)) => {
                if price_differs(item, current_price) {
                    (
                        PriceApplyResult::Apply,
                        "price change — proposed to apply".to_string(),
                    )
                } else {
                    (
                        PriceApplyResult::Skipped,
                        "already at catalog price".to_string(),
                    )
                }
            }
            (CatalogAction::Change, None) => (
                PriceApplyResult::Skipped,
                "not carried — nothing to change".to_string(),
            ),
            (CatalogAction::Drop, Some(_)) => (
                PriceApplyResult::Drop,
                "vendor discontinued — proposed to deactivate".to_string(),
            ),
            (CatalogAction::Drop, None) => (
                PriceApplyResult::Skipped,
                "not carried — nothing to drop".to_string(),
            ),
        };
        lines.push(AppliedPriceLine {
            vendor_sku: item.vendor_sku.clone(),
            action: item.action,
            old_cost: current_price.map(|price| price.cost),
            old_list_price: current_price.map(|price| price.list_price),
            new_cost: item.cost.or_else(|| current_price.map(|price| price.cost)),
            new_list_price: item
                .list_price
                .or_else(|| current_price.map(|price| price.list_price)),
            result,
            note,
        });
    }
    PriceCatalogApplication {
        id: Uuid::now_v7(),
        vendor_id: catalog.vendor_id.clone(),
        catalog_id: catalog.catalog_id.clone(),
        catalog_date: catalog.catalog_date,
        status: ApplicationStatus::PendingReview,
        reviewed_by: None,
        lines,
    }
}

fn price_differs(item: &PriceCatalogItem, current: Option<&CurrentPrice>) -> bool {
    let Some(current) = current else {
        return true;
    };
    item.cost.is_some_and(|cost| cost != current.cost)
        || item
            .list_price
            .is_some_and(|price| price != current.list_price)
}

impl PriceCatalogApplication {
    /// Mark the report applied after a human review. This only
    /// records the review decision on the report — the caller still
    /// owns writing the new prices to the product catalog.
    pub fn mark_applied(&mut self, reviewed_by: &str) {
        self.status = ApplicationStatus::Applied;
        self.reviewed_by = Some(reviewed_by.to_string());
    }

    /// Number of lines proposed to change the catalog (`Apply` /
    /// `Add` / `Drop`).
    pub fn proposed_change_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| line.result != PriceApplyResult::Skipped)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn sample_catalog() -> PriceCatalog {
        PriceCatalog {
            vendor_id: "VENDOR-A".to_string(),
            catalog_id: "CAT-2026-11".to_string(),
            catalog_date: NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(),
            items: vec![
                PriceCatalogItem {
                    vendor_sku: "SKU-A".to_string(),
                    description: "Interior Eggshell Gallon".to_string(),
                    cost: Some(dec("26.0000")),
                    list_price: Some(dec("52.9900")),
                    uom: "GAL".to_string(),
                    action: CatalogAction::Change,
                    effective_date: NaiveDate::from_ymd_opt(2026, 12, 1),
                },
                PriceCatalogItem {
                    vendor_sku: "SKU-NEW".to_string(),
                    description: "New Primer Quart".to_string(),
                    cost: Some(dec("9.2500")),
                    list_price: Some(dec("18.4900")),
                    uom: "QT".to_string(),
                    action: CatalogAction::Add,
                    effective_date: None,
                },
                PriceCatalogItem {
                    vendor_sku: "SKU-OLD".to_string(),
                    description: "Discontinued Stain".to_string(),
                    cost: None,
                    list_price: None,
                    uom: "GAL".to_string(),
                    action: CatalogAction::Drop,
                    effective_date: None,
                },
            ],
        }
    }

    #[test]
    fn catalog_832_round_trip() {
        let catalog = sample_catalog();
        let text = price_catalog_to_x12_832(&catalog, "VENDOR-A", "DECORPOS", 11);
        assert!(text.contains("ST*832*11~"));
        assert!(text.contains("BCT*00*CAT-2026-11*20261125~"));
        assert!(text.contains("LIN*1*VP*SKU-A~"));
        assert!(text.contains("CTP*LIST*52.9900~"));
        assert!(text.contains("CTP*COST*26.0000~"));
        assert!(text.contains("REF*ACTION*CHG~"));
        assert!(text.contains("REF*ACTION*ADD~"));
        assert!(text.contains("REF*ACTION*DROP~"));
        assert!(text.contains("DTM*007*20261201~"));
        let back = parse_832(&text).unwrap();
        assert_eq!(back, catalog);
    }

    #[test]
    fn apply_report_never_silently_overwrites() {
        let catalog = sample_catalog();
        let current = vec![
            CurrentPrice {
                vendor_sku: "SKU-A".to_string(),
                cost: dec("24.5000"),
                list_price: dec("49.9900"),
            },
            CurrentPrice {
                vendor_sku: "SKU-OLD".to_string(),
                cost: dec("15.0000"),
                list_price: dec("29.9900"),
            },
            CurrentPrice {
                vendor_sku: "SKU-SAME".to_string(),
                cost: dec("10.0000"),
                list_price: dec("20.0000"),
            },
        ];
        let mut report = build_price_apply_report(&catalog, &current);
        assert_eq!(report.status, ApplicationStatus::PendingReview);
        assert_eq!(report.lines.len(), 3);
        // SKU-A: price changed -> Apply, before/after shown.
        assert_eq!(report.lines[0].result, PriceApplyResult::Apply);
        assert_eq!(report.lines[0].old_cost, Some(dec("24.5000")));
        assert_eq!(report.lines[0].new_cost, Some(dec("26.0000")));
        assert_eq!(report.lines[0].old_list_price, Some(dec("49.9900")));
        assert_eq!(report.lines[0].new_list_price, Some(dec("52.9900")));
        // SKU-NEW: unknown -> Add.
        assert_eq!(report.lines[1].result, PriceApplyResult::Add);
        assert_eq!(report.lines[1].old_cost, None);
        // SKU-OLD: carried -> Drop.
        assert_eq!(report.lines[2].result, PriceApplyResult::Drop);
        assert_eq!(report.proposed_change_count(), 3);

        report.mark_applied("manager@store");
        assert_eq!(report.status, ApplicationStatus::Applied);
        assert_eq!(report.reviewed_by.as_deref(), Some("manager@store"));
    }

    #[test]
    fn apply_report_skips_identical_and_unknown() {
        let mut catalog = sample_catalog();
        catalog.items[0].cost = Some(dec("24.5000"));
        catalog.items[0].list_price = Some(dec("49.9900"));
        catalog.items[2].vendor_sku = "SKU-NEVER-CARRIED".to_string();
        let current = vec![CurrentPrice {
            vendor_sku: "SKU-A".to_string(),
            cost: dec("24.5000"),
            list_price: dec("49.9900"),
        }];
        let report = build_price_apply_report(&catalog, &current);
        assert_eq!(report.lines[0].result, PriceApplyResult::Skipped);
        assert_eq!(report.lines[2].result, PriceApplyResult::Skipped);
        assert_eq!(report.proposed_change_count(), 1); // only the Add
    }

    #[test]
    fn parse_832_rejects_missing_action_and_bad_set() {
        assert!(parse_832("ST*850*1~BCT*00*C1*20261125~LIN*1*VP*SKU-A~").is_err());
        assert!(parse_832("ST*832*1~BCT*00*C1*20261125~LIN*1*VP*SKU-A~PID*F****Thing~").is_err()); // no REF*ACTION
        assert!(parse_832("ST*832*1~BCT*00*C1*20261125~CTP*LIST*1.0000~").is_err());
    }
}
