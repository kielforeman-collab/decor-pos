//! Phase 4 `df-db` repositories (migrations `0006_phase4` +
//! `0007_phase4_crm_delivery`): the `df-delivery` dispatch /
//! POD-gated close-out rules, the `df-crm` tickler / overlap /
//! lapsed-report / saved-view rules — all against the SQLite edge
//! schema.
//!
//! Exit test (b): a delivery moves dispatch → out-for-delivery →
//! delivered ONLY with a recorded signature + photo reference,
//! and close-out without them fails with nothing posted.
//! Exit test (c): the lapsed-contractor report lists a contractor
//! with no purchase in the window and omits one with a recent
//! purchase, with never-purchased contractors as a distinct
//! status.

use chrono::{NaiveDate, TimeDelta, Utc};
use df_crm::{AppointmentStatus, CustomerFilter, LapsedStatus};
use df_db::models::{
    Appointment, ArLedgerEntry, CrmActivity, CrmSavedView, Customer, Delivery, DeliveryPod,
    Organization, Store, Transaction, TransactionLine,
};
use df_delivery::{DeliveryStatus, PodPhoto, PodRecord, PodSignature, sha256_hex};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn test_pool() -> df_db::AnyPool {
    let pool = df_db::connect("sqlite::memory:").await.unwrap();
    df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
    pool
}

fn org() -> Organization {
    Organization {
        id: Uuid::now_v7(),
        name: "Test Paint Co".to_string(),
        default_currency: "USD".to_string(),
        enabled_modules: serde_json::json!({}),
        settings_json: serde_json::json!({}),
        created_at: Utc::now(),
    }
}

fn store(org_id: Uuid) -> Store {
    Store {
        id: Uuid::now_v7(),
        org_id,
        code: "MAIN".to_string(),
        name: "Main Store".to_string(),
        country_code: "US".to_string(),
        region_code: "WA".to_string(),
        postal_code: "98101".to_string(),
        currency: "USD".to_string(),
        tax_zone_id: Uuid::now_v7(),
        timezone: "America/Los_Angeles".to_string(),
        is_warehouse: false,
    }
}

fn customer(org_id: Uuid, number: &str, ctype: &str) -> Customer {
    Customer {
        id: Uuid::now_v7(),
        org_id,
        account_number: number.to_string(),
        customer_type: ctype.to_string(),
        company_name: Some(format!("{number} Painting")),
        first_name: None,
        last_name: None,
        email: None,
        phone: None,
        price_level_id: None,
        salesperson_id: None,
        tax_exempt_cert: None,
        tax_exempt_expires: None,
        charge_enabled: true,
        credit_limit: dec("5000.0000"),
        payment_terms: "NET30".to_string(),
        terms_days: Some(30),
        finance_rate_pct_monthly: Some(dec("1.500")),
        credit_hold: false,
        require_po_number: false,
        require_job_name: false,
        portal_enabled: false,
    }
}

fn sale(
    org_id: Uuid,
    store_id: Uuid,
    customer_id: Uuid,
    doc: &str,
    total: &str,
    days_ago: i64,
) -> Transaction {
    Transaction {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        till_session_id: None,
        doc_number: doc.to_string(),
        doc_type: "sale".to_string(),
        status: "completed".to_string(),
        customer_id: Some(customer_id),
        job_id: None,
        po_number: None,
        salesperson_id: None,
        cashier_id: None,
        currency: "USD".to_string(),
        subtotal: dec(total),
        tax_total: dec("0.0000"),
        eco_fee_total: dec("0.0000"),
        pos_fee_total: dec("0.0000"),
        grand_total: dec(total),
        deposit_paid: dec("0.0000"),
        fulfillment_mode: "counter".to_string(),
        scheduled_at: None,
        completed_at: Some(Utc::now() - TimeDelta::days(days_ago)),
        created_at: Utc::now() - TimeDelta::days(days_ago),
        job_name: None,
    }
}

fn delivery(org_id: Uuid, store_id: Uuid, transaction_id: Option<Uuid>, priority: i32) -> Delivery {
    Delivery {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        transaction_id,
        customer_id: None,
        driver_id: None,
        status: DeliveryStatus::Awaiting.as_str().to_string(),
        address_json: serde_json::json!({
            "street": "12 Main St",
            "city": "Vancouver",
            "region": "BC",
            "postal_code": "V6B 1A1",
        }),
        scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 11, 27).unwrap()),
        priority,
        vehicle: Some("Van 2".to_string()),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

/// A POD row for `delivery_id`, built from a domain `PodRecord`
/// so the stored base64 + hashes are exactly what the domain
/// crate produces.
fn pod_row(delivery_id: Uuid, photo: bool) -> DeliveryPod {
    let now = Utc::now();
    let mut record = PodRecord::new(delivery_id);
    record.record_signature(
        PodSignature::capture(b"fake-signature-png".to_vec(), "Dana Driver", now).unwrap(),
        now,
    );
    if photo {
        record.record_photo(
            PodPhoto::record(
                "pod/2026-11-25/delivery-1.jpg",
                sha256_hex(b"fake-photo-jpg"),
                now,
            )
            .unwrap(),
            now,
        );
    }
    let sig = record.signature.unwrap();
    let signature_b64 = sig.to_b64();
    let (photo_ref, photo_sha256, photo_at) = match record.photo {
        Some(p) => (Some(p.path), Some(p.sha256_hex), Some(p.captured_at)),
        None => (None, None, None),
    };
    DeliveryPod {
        id: record.id,
        delivery_id,
        signature_sha256: sig.sha256_hex,
        signature_b64,
        photo_ref,
        photo_sha256,
        received_by: sig.signer_name,
        signed_at: sig.captured_at,
        photo_at,
        created_at: now,
    }
}

// --- exit test (b): POD-gated close-out -------------------------------------

#[tokio::test]
async fn exit_b_pod_close_out_requires_signature_and_photo() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = store(org.id);
    df_db::insert_store(&pool, &store).await.unwrap();
    let cust = customer(org.id, "C-1001", "contractor");
    df_db::insert_customer(&pool, &cust).await.unwrap();

    // An open order awaiting delivery.
    let mut order = sale(org.id, store.id, cust.id, "ORD-1001", "250.0000", 0);
    order.doc_type = "order".to_string();
    order.status = "open".to_string();
    order.completed_at = None;
    df_db::insert_transaction(&pool, &order).await.unwrap();
    // One line, so the packing slip has content.
    df_db::insert_transaction_line(
        &pool,
        &TransactionLine {
            id: Uuid::now_v7(),
            transaction_id: order.id,
            line_no: 1,
            product_id: Uuid::now_v7(),
            description: "Eggshell White — 1 gal".to_string(),
            qty: dec("2.0000"),
            uom_code: "gal".to_string(),
            unit_price: dec("125.0000"),
            base_unit_cost: dec("80.0000"),
            tint_unit_cost: dec("0.0000"),
            discount_amount: dec("0.0000"),
            tax_amount: dec("0.0000"),
            eco_fee_amount: dec("0.0000"),
            line_total: dec("250.0000"),
            tint_formula_id: None,
            prompts_json: serde_json::json!({}),
            vertical_spec_json: serde_json::json!({}),
            linked_po_line_id: None,
            fulfilled_qty: dec("0.0000"),
        },
    )
    .await
    .unwrap();

    let actor = Uuid::now_v7();
    let now = Utc::now();
    let d = delivery(org.id, store.id, Some(order.id), 5);
    df_db::insert_delivery(&pool, &d).await.unwrap();

    // Dispatch: draft -> assigned -> out_for_delivery.
    df_db::transition_delivery(&pool, d.id, DeliveryStatus::Assigned, actor, None, now)
        .await
        .unwrap();
    df_db::transition_delivery(
        &pool,
        d.id,
        DeliveryStatus::OutForDelivery,
        actor,
        None,
        now,
    )
    .await
    .unwrap();
    // A direct jump to delivered is refused by the transition path.
    assert!(matches!(
        df_db::transition_delivery(&pool, d.id, DeliveryStatus::Delivered, actor, None, now).await,
        Err(df_db::DbError::IllegalDeliveryTransition { .. })
    ));
    // Skipping a state is refused too (fresh delivery, draft -> out_for_delivery).
    let d2 = delivery(org.id, store.id, None, 1);
    df_db::insert_delivery(&pool, &d2).await.unwrap();
    assert!(matches!(
        df_db::transition_delivery(
            &pool,
            d2.id,
            DeliveryStatus::OutForDelivery,
            actor,
            None,
            now
        )
        .await,
        Err(df_db::DbError::IllegalDeliveryTransition { .. })
    ));

    // No POD at all: close-out fails, nothing posted.
    assert!(matches!(
        df_db::close_out_delivery(&pool, d.id, actor, now).await,
        Err(df_db::DbError::PodIncomplete { .. })
    ));
    let back = df_db::get_delivery(&pool, d.id).await.unwrap();
    assert_eq!(back.status, "out_for_delivery");
    assert_eq!(
        df_db::list_delivery_events_for_delivery(&pool, d.id)
            .await
            .unwrap()
            .len(),
        2
    );

    // Signature only: still incomplete, still nothing posted.
    df_db::insert_delivery_pod(&pool, &pod_row(d.id, false))
        .await
        .unwrap();
    let err = df_db::close_out_delivery(&pool, d.id, actor, now)
        .await
        .unwrap_err();
    assert!(matches!(err, df_db::DbError::PodIncomplete { ref missing, .. } if missing == "photo"));
    let back = df_db::get_delivery(&pool, d.id).await.unwrap();
    assert_eq!(back.status, "out_for_delivery");
    assert_eq!(
        df_db::list_delivery_events_for_delivery(&pool, d.id)
            .await
            .unwrap()
            .len(),
        2
    );
    // The order is still open — nothing was auto-closed.
    assert_eq!(
        df_db::get_transaction(&pool, order.id)
            .await
            .unwrap()
            .status,
        "open"
    );

    // Signature + photo: close-out succeeds and auto-closes the order.
    let mut complete_pod = pod_row(d.id, true);
    complete_pod.created_at = Utc::now();
    df_db::insert_delivery_pod(&pool, &complete_pod)
        .await
        .unwrap();
    let event = df_db::close_out_delivery(&pool, d.id, actor, now)
        .await
        .unwrap();
    assert_eq!(event.to_status, "delivered");
    assert_eq!(event.from_status, "out_for_delivery");
    let back = df_db::get_delivery(&pool, d.id).await.unwrap();
    assert_eq!(back.status, "delivered");
    assert_eq!(
        df_db::list_delivery_events_for_delivery(&pool, d.id)
            .await
            .unwrap()
            .len(),
        3
    );
    let closed_order = df_db::get_transaction(&pool, order.id).await.unwrap();
    assert_eq!(closed_order.status, "completed");
    assert!(closed_order.completed_at.is_some());
    // Delivered is terminal: a second close-out fails.
    assert!(
        df_db::close_out_delivery(&pool, d.id, actor, now)
            .await
            .is_err()
    );

    // Packing slip renders from the linked sale and persists.
    let slip = df_db::render_and_store_packing_slip(&pool, d.id, "Test Paint Co")
        .await
        .unwrap();
    assert!(slip.rendered_text.contains("ORD-1001"));
    assert!(slip.rendered_text.contains("2 x gal Eggshell White"));
    assert!(slip.rendered_text.contains("Total items: 2"));
    assert!(slip.rendered_zpl.starts_with("^XA\n"));
    let stored = df_db::get_packing_slip_for_delivery(&pool, d.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.id, slip.id);

    // Dispatch board: the delivered order is gone; the untouched
    // draft delivery remains.
    let board = df_db::list_dispatch_board(&pool, store.id).await.unwrap();
    assert_eq!(board.len(), 1);
    assert_eq!(board[0].id, d2.id);
}

// --- exit test (c): lapsed-contractor report --------------------------------

#[tokio::test]
async fn exit_c_lapsed_contractor_report() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = store(org.id);
    df_db::insert_store(&pool, &store).await.unwrap();

    let lapsed = customer(org.id, "C-2001", "contractor");
    let recent = customer(org.id, "C-2002", "contractor");
    let never = customer(org.id, "C-2003", "contractor");
    let retail = customer(org.id, "C-2004", "retail");
    for c in [&lapsed, &recent, &never, &retail] {
        df_db::insert_customer(&pool, c).await.unwrap();
    }
    // Lapsed: last sale 200 days ago. Recent: 10 days ago.
    // Retail: 300 days ago — old, but not a contractor.
    df_db::insert_transaction(
        &pool,
        &sale(org.id, store.id, lapsed.id, "S-1", "500.0000", 200),
    )
    .await
    .unwrap();
    df_db::insert_transaction(
        &pool,
        &sale(org.id, store.id, recent.id, "S-2", "1250.0000", 10),
    )
    .await
    .unwrap();
    df_db::insert_transaction(
        &pool,
        &sale(org.id, store.id, retail.id, "S-3", "99.0000", 300),
    )
    .await
    .unwrap();
    // An open AR charge for the lapsed contractor (balance $250).
    df_db::insert_ar_entry(
        &pool,
        &ArLedgerEntry {
            id: Uuid::now_v7(),
            org_id: org.id,
            store_id: store.id,
            customer_id: lapsed.id,
            job_id: None,
            entry_type: "charge".to_string(),
            reference_type: "transaction".to_string(),
            reference_id: Uuid::now_v7(),
            amount: dec("250.0000"),
            due_date: None,
            statement_id: None,
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();

    let now = Utc::now();
    let report = df_db::lapsed_contractor_report(&pool, org.id, now, 90, true)
        .await
        .unwrap();
    // Never-purchased first, then the lapsed contractor. The
    // recent buyer and the retail customer are absent.
    assert_eq!(report.len(), 2);
    assert_eq!(report[0].status, LapsedStatus::NeverPurchased);
    assert_eq!(report[0].account_number, "C-2003");
    assert_eq!(report[0].last_purchase_at, None);
    assert_eq!(report[1].status, LapsedStatus::Lapsed);
    assert_eq!(report[1].account_number, "C-2001");
    assert!(report[1].last_purchase_at.is_some());
    assert_eq!(report[1].trailing_spend, dec("500.0000"));
    assert_eq!(report[1].ar_balance, dec("250.0000"));

    // Excluding never-purchased drops only that row.
    let report = df_db::lapsed_contractor_report(&pool, org.id, now, 90, false)
        .await
        .unwrap();
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].account_number, "C-2001");
}

// --- tickler -----------------------------------------------------------------

fn activity(store_id: Uuid, org_id: Uuid, due_in_hours: i64) -> CrmActivity {
    CrmActivity {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        customer_id: None,
        activity_type: "follow_up".to_string(),
        subject: "Chase the Miller quote".to_string(),
        notes: None,
        created_by: None,
        due_at: Some(Utc::now() + TimeDelta::hours(due_in_hours)),
        completed_at: None,
        snoozed_until: None,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn tickler_due_overdue_complete_snooze() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = store(org.id);
    df_db::insert_store(&pool, &store).await.unwrap();
    let now = Utc::now();
    let overdue = activity(store.id, org.id, -3);
    let future = activity(store.id, org.id, 5);
    df_db::insert_crm_activity(&pool, &overdue).await.unwrap();
    df_db::insert_crm_activity(&pool, &future).await.unwrap();

    let due = df_db::list_reminders_due_for_store(&pool, store.id, now)
        .await
        .unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, overdue.id);
    let over = df_db::list_overdue_reminders_for_store(&pool, store.id, now)
        .await
        .unwrap();
    assert_eq!(over.len(), 1);

    // Completing removes it from both lists.
    let done = df_db::complete_reminder(&pool, overdue.id, now)
        .await
        .unwrap();
    assert!(done.completed_at.is_some());
    assert!(
        df_db::list_reminders_due_for_store(&pool, store.id, now)
            .await
            .unwrap()
            .is_empty()
    );

    // Snoozing the future one past now keeps it out of the due list.
    let snoozed = df_db::snooze_reminder(&pool, future.id, now + TimeDelta::hours(30), now)
        .await
        .unwrap();
    assert_eq!(snoozed.snoozed_until, Some(now + TimeDelta::hours(30)));
    assert!(
        df_db::list_reminders_due_for_store(&pool, store.id, now + TimeDelta::hours(6))
            .await
            .unwrap()
            .is_empty()
    );
}

// --- appointment overlap -----------------------------------------------------

fn appointment(
    org_id: Uuid,
    store_id: Uuid,
    customer_id: Option<Uuid>,
    staff_id: Option<Uuid>,
    start_hour: i64,
    end_hour: i64,
) -> Appointment {
    let base = Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    Appointment {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        customer_id,
        staff_id,
        title: "Measure".to_string(),
        starts_at: base + TimeDelta::hours(start_hour),
        ends_at: base + TimeDelta::hours(end_hour),
        status: AppointmentStatus::Scheduled.as_str().to_string(),
        notes: None,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn appointment_overlap_checked_on_insert() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = store(org.id);
    df_db::insert_store(&pool, &store).await.unwrap();
    let contractor = Uuid::now_v7();
    let staff = Uuid::now_v7();

    let first = appointment(org.id, store.id, Some(contractor), Some(staff), 9, 11);
    df_db::insert_appointment_checked(&pool, &first)
        .await
        .unwrap();
    // Same contractor, overlapping: rejected, nothing inserted.
    let clash = appointment(
        org.id,
        store.id,
        Some(contractor),
        Some(Uuid::now_v7()),
        10,
        12,
    );
    assert!(matches!(
        df_db::insert_appointment_checked(&pool, &clash).await,
        Err(df_db::DbError::AppointmentOverlap { .. })
    ));
    // Same staff, different contractor: also rejected.
    let staff_clash = appointment(org.id, store.id, Some(Uuid::now_v7()), Some(staff), 10, 12);
    assert!(
        df_db::insert_appointment_checked(&pool, &staff_clash)
            .await
            .is_err()
    );
    // Adjacent slot for the same contractor: fine.
    let adjacent = appointment(org.id, store.id, Some(contractor), Some(staff), 11, 12);
    df_db::insert_appointment_checked(&pool, &adjacent)
        .await
        .unwrap();
    // Cancelling the first AND the adjacent booking frees the slot.
    let mut cancelled = first.clone();
    cancelled.status = "cancelled".to_string();
    df_db::update_appointment(&pool, &cancelled).await.unwrap();
    let mut cancelled_adjacent = adjacent.clone();
    cancelled_adjacent.status = "cancelled".to_string();
    df_db::update_appointment(&pool, &cancelled_adjacent)
        .await
        .unwrap();
    df_db::insert_appointment_checked(&pool, &clash)
        .await
        .unwrap();

    // Day listing: all three stored rows start today.
    let all = df_db::list_appointments_for_store(&pool, store.id)
        .await
        .unwrap();
    assert_eq!(all.len(), 3);
}

// --- saved-view evaluation ---------------------------------------------------

#[tokio::test]
async fn saved_view_customer_filter_evaluation() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = store(org.id);
    df_db::insert_store(&pool, &store).await.unwrap();
    let big = customer(org.id, "C-3001", "contractor");
    let small = customer(org.id, "C-3002", "contractor");
    let retail = customer(org.id, "C-3003", "retail");
    for c in [&big, &small, &retail] {
        df_db::insert_customer(&pool, c).await.unwrap();
    }
    // Only the big contractor carries a balance.
    df_db::insert_ar_entry(
        &pool,
        &ArLedgerEntry {
            id: Uuid::now_v7(),
            org_id: org.id,
            store_id: store.id,
            customer_id: big.id,
            job_id: None,
            entry_type: "charge".to_string(),
            reference_type: "transaction".to_string(),
            reference_id: Uuid::now_v7(),
            amount: dec("1500.0000"),
            due_date: None,
            statement_id: None,
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();

    let filter = CustomerFilter {
        customer_type: Some("contractor".to_string()),
        min_balance: Some(dec("1000.0000")),
        ..Default::default()
    };
    let view = CrmSavedView {
        id: Uuid::now_v7(),
        org_id: org.id,
        store_id: store.id,
        name: "Big-balance contractors".to_string(),
        filter_json: serde_json::to_value(&filter).unwrap(),
        created_at: Utc::now(),
    };
    df_db::insert_crm_saved_view(&pool, &view).await.unwrap();
    let hits = df_db::evaluate_saved_view(&pool, &view).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, big.id);

    // A view stored in the activity-filter dialect is rejected,
    // not silently misread as a customer filter.
    let activity_view = CrmSavedView {
        id: Uuid::now_v7(),
        org_id: org.id,
        store_id: store.id,
        name: "Open calls".to_string(),
        filter_json: serde_json::json!({"activity_type": "call", "completed": false}),
        created_at: Utc::now(),
    };
    assert!(matches!(
        df_db::evaluate_saved_view(&pool, &activity_view).await,
        Err(df_db::DbError::InvalidData(_))
    ));
}
