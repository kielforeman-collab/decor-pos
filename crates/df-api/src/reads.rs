//! HQ read-only views (Phases 3–4): inventory balances, AR summary & aging,
//! CRM activities/appointments, delivery dispatch, and EDI documents.

use std::str::FromStr;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::NaiveDate;
use df_db::InventoryBalance;
use df_domain::Currency;
use df_domain::ar::{ArEntry, aging_totals};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::schemas::{
    AgingCustomerResponse, AgingResponse, AppointmentListQuery, ArAgingQuery,
    ArCustomerSummaryResponse, DispatchQuery, EdiDocumentsQuery, StoreQuery,
};
use crate::{ApiResult, ApiState, db_err, edi_err, err, org_id, pool, report_date, store_id};

// --- Phase 3 inventory + AR reads (spec §6.1) -------------------------------
// Read-only HQ mirror of the edge back-office surface: balances are
// derived from the append-only ledger by `df-db`, aging buckets by
// `df-domain::ar` — the same helpers the edge endpoints use, so HQ
// and edge agree. Mutating purchasing / transfer / count endpoints
// stay edge-only (they need the edge's store context and ledger
// transactions); the HQ API deliberately does not duplicate them.

/// Stock positions for every product at a store.
pub async fn inventory_balances(
    State(state): State<ApiState>,
    Query(query): Query<StoreQuery>,
) -> ApiResult<Vec<InventoryBalance>> {
    let pool = pool(&state)?;
    let org = org_id(&state)?;
    let store = store_id(query.store_id, &state)?;
    df_db::inventory_balances(&pool, org, store)
        .await
        .map(Json)
        .map_err(db_err)
}

/// One customer's AR position: ledger balance, credit controls and
/// entry count.
pub async fn ar_customer_summary(
    State(state): State<ApiState>,
    Path(customer_id): Path<Uuid>,
) -> ApiResult<ArCustomerSummaryResponse> {
    let pool = pool(&state)?;
    let customer = df_db::get_customer(&pool, customer_id)
        .await
        .map_err(db_err)?;
    let entries = df_db::list_ar_entries_for_customer(&pool, customer_id)
        .await
        .map_err(db_err)?;
    let balance = entries
        .iter()
        .map(|entry| entry.amount)
        .fold(Decimal::ZERO, |acc, amount| acc + amount);
    let oldest_due_date = entries.iter().filter_map(|entry| entry.due_date).min();
    Ok(Json(ArCustomerSummaryResponse {
        customer_id: customer.id.to_string(),
        account_number: customer.account_number,
        company_name: customer.company_name,
        customer_type: customer.customer_type,
        charge_enabled: customer.charge_enabled,
        credit_limit: customer.credit_limit.to_string(),
        credit_hold: customer.credit_hold,
        balance: balance.to_string(),
        entry_count: entries.len(),
        oldest_due_date,
    }))
}

/// 30/60/90/120 aging for every customer with AR entries at a
/// store. Same bucketing rule as the edge endpoint: positive entries
/// bucket by due date (no due date = current); payments and credit
/// memos (negative amounts) are unallocated and reduce the Current
/// bucket, so the bucket sum always equals the balance.
pub async fn ar_aging(
    State(state): State<ApiState>,
    Query(query): Query<ArAgingQuery>,
) -> ApiResult<AgingResponse> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    let as_of = report_date(query.as_of)?;
    let store_row = df_db::get_store(&pool, store).await.map_err(db_err)?;
    let currency = Currency::from_str(&store_row.currency)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let entries = df_db::list_ar_entries_for_store(&pool, store)
        .await
        .map_err(db_err)?;
    let mut by_customer: std::collections::HashMap<Uuid, Vec<df_db::models::ArLedgerEntry>> =
        std::collections::HashMap::new();
    for entry in entries {
        by_customer
            .entry(entry.customer_id)
            .or_default()
            .push(entry);
    }
    let mut customers: Vec<AgingCustomerResponse> = Vec::new();
    let mut totals = [
        df_domain::Money::zero(currency),
        df_domain::Money::zero(currency),
        df_domain::Money::zero(currency),
        df_domain::Money::zero(currency),
        df_domain::Money::zero(currency),
    ];
    for (customer_id, customer_entries) in &by_customer {
        let customer = df_db::get_customer(&pool, *customer_id)
            .await
            .map_err(db_err)?;
        let mut open: Vec<ArEntry> = Vec::new();
        let mut credits = df_domain::Money::zero(currency);
        for entry in customer_entries {
            if entry.amount >= Decimal::ZERO {
                open.push(ArEntry {
                    id: entry.id,
                    customer_id: entry.customer_id,
                    job_id: entry.job_id,
                    balance: df_domain::Money::new(entry.amount, currency),
                    due_date: entry.due_date.unwrap_or(entry.created_at.date_naive()),
                });
            } else {
                credits = credits
                    .checked_add(df_domain::Money::new(entry.amount, currency))
                    .unwrap_or(credits);
            }
        }
        let mut buckets = aging_totals(&open, as_of, currency);
        if !credits.is_zero() {
            buckets[0] = buckets[0].checked_add(credits).unwrap_or(buckets[0]);
        }
        let balance = customer_entries
            .iter()
            .map(|entry| entry.amount)
            .fold(Decimal::ZERO, |acc, amount| acc + amount);
        for (index, bucket) in buckets.iter().enumerate() {
            totals[index] = totals[index].checked_add(*bucket).unwrap_or(totals[index]);
        }
        customers.push(AgingCustomerResponse {
            customer_id: customer_id.to_string(),
            account_number: customer.account_number,
            balance: balance.to_string(),
            buckets: buckets
                .iter()
                .map(|bucket| bucket.amount().to_string())
                .collect::<Vec<_>>()
                .try_into()
                .expect("aging_totals returns 5 buckets"),
        });
    }
    customers.sort_by(|a, b| a.account_number.cmp(&b.account_number));
    Ok(Json(AgingResponse {
        as_of,
        totals: totals
            .iter()
            .map(|total| total.amount().to_string())
            .collect::<Vec<_>>()
            .try_into()
            .expect("aging_totals returns 5 buckets"),
        customers,
    }))
}

// --- Phase 4 HQ reads -------------------------------------------------------
// Read-only HQ views of the edge's CRM, delivery and EDI rows
// (spec §6.1/§7). Writes stay on the edge; the lapsed-contractor
// report is edge-only (see the module docs).

/// CRM activity log for a store, newest first (same rows as the
/// edge's `GET /crm/activities`, no filters on the HQ read).
pub async fn crm_activities(
    State(state): State<ApiState>,
    Query(query): Query<StoreQuery>,
) -> ApiResult<Vec<df_db::models::CrmActivity>> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    df_db::list_crm_activities_for_store(&pool, store)
        .await
        .map(Json)
        .map_err(db_err)
}

/// Appointments for a store, earliest first, optionally on one
/// UTC calendar day (`?date=YYYY-MM-DD`).
pub async fn crm_appointments(
    State(state): State<ApiState>,
    Query(query): Query<AppointmentListQuery>,
) -> ApiResult<Vec<df_db::models::Appointment>> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    let day = match query.date {
        None => None,
        Some(text) => Some(NaiveDate::from_str(&text).map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                format!("date must be YYYY-MM-DD, got {text:?}"),
            )
        })?),
    };
    let mut appointments = df_db::list_appointments_for_store(&pool, store)
        .await
        .map_err(db_err)?;
    if let Some(day) = day {
        appointments.retain(|a| a.starts_at.date_naive() == day);
    }
    appointments.sort_by_key(|a| a.starts_at);
    Ok(Json(appointments))
}

/// The dispatch board for a store: open deliveries, highest
/// priority first, then oldest first. An optional `?status=`
/// filters the board (storage strings: `draft`, `assigned`,
/// `out_for_delivery`, `delivered`, `cancelled` — an unknown
/// status is a 400).
pub async fn delivery_dispatch(
    State(state): State<ApiState>,
    Query(query): Query<DispatchQuery>,
) -> ApiResult<Vec<df_db::models::Delivery>> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    if let Some(status) = &query.status
        && !matches!(
            status.as_str(),
            "draft" | "assigned" | "out_for_delivery" | "delivered" | "cancelled"
        )
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            format!(
                "unknown delivery status {status:?} (draft, assigned, out_for_delivery, delivered, cancelled)"
            ),
        ));
    }
    let mut deliveries = df_db::list_dispatch_board(&pool, store)
        .await
        .map_err(db_err)?;
    if let Some(status) = query.status {
        deliveries.retain(|d| d.status == status);
    }
    Ok(Json(deliveries))
}

/// EDI documents linked to a PO, newest first. The PO must exist
/// (unknown ids are a 404).
pub async fn edi_documents(
    State(state): State<ApiState>,
    Query(query): Query<EdiDocumentsQuery>,
) -> ApiResult<Vec<df_edi::EdiDocument>> {
    let pool = pool(&state)?;
    df_edi::repos::get_purchase_order(&pool, query.po_id)
        .await
        .map_err(edi_err)?;
    df_edi::repos::list_edi_documents_for_po(&pool, query.po_id)
        .await
        .map(Json)
        .map_err(edi_err)
}
