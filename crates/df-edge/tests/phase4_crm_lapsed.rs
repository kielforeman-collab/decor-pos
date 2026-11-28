//! Phase 4 exit test (CRM + lapsed report): lapsed vs
//! never-purchased vs recent vs non-contractor classification,
//! tickler complete/snooze rules, appointment overlap (same
//! contractor OR same staff), and both saved-view dialects.
//!
//! Uses the REAL crates end to end — real checkouts for the
//! purchases (backdated by one timestamp update, so the sale
//! itself is honest), the `df-crm` tickler / overlap / filter
//! rules via the edge's CRM wiring — against an in-memory
//! SQLite edge.

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use df_db::models::{Customer, Organization, Product, Store};
use df_domain::{Currency, Money};
use df_edge::{
    CreateActivityRequest, CreateAppointmentRequest, CreateSavedViewRequest, Edge, EdgeConfig,
    MockTerminal, SaleInput, SaleLineInput, TenderInput, TenderKind,
};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn money(s: &str) -> Money {
    Money::from_str_amount(s, Currency::Usd).unwrap()
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const PRODUCT_ID: u128 = 0x3000;
const CUSTOMER_A: u128 = 0x4001; // contractor, bought 100 days ago
const CUSTOMER_B: u128 = 0x4002; // contractor, never bought
const CUSTOMER_C: u128 = 0x4003; // contractor, bought today
const CUSTOMER_D: u128 = 0x4004; // retail, bought 100 days ago
const STAFF_S: u128 = 0x5001;
const STAFF_T: u128 = 0x5002;

fn customer(id: Uuid, org_id: Uuid, number: &str, kind: &str) -> Customer {
    Customer {
        id,
        org_id,
        account_number: number.to_string(),
        customer_type: kind.to_string(),
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
        credit_limit: dec("5000"),
        payment_terms: "NET30".to_string(),
        terms_days: None,
        finance_rate_pct_monthly: None,
        credit_hold: false,
        require_po_number: false,
        require_job_name: false,
        portal_enabled: false,
    }
}

async fn test_edge() -> Edge {
    let config = EdgeConfig {
        store_id: Uuid::from_u128(STORE_ID),
        org_id: Uuid::from_u128(ORG_ID),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Test Paint Co".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    };
    let edge = Edge::open(config).await.unwrap();
    let pool = edge.pool();
    df_db::insert_organization(
        pool,
        &Organization {
            id: Uuid::from_u128(ORG_ID),
            name: "Test Paint Co".to_string(),
            default_currency: "USD".to_string(),
            enabled_modules: serde_json::json!({}),
            settings_json: serde_json::json!({}),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    df_db::insert_store(
        pool,
        &Store {
            id: Uuid::from_u128(STORE_ID),
            org_id: Uuid::from_u128(ORG_ID),
            code: "MAIN".to_string(),
            name: "Main Store".to_string(),
            country_code: "US".to_string(),
            region_code: "CA".to_string(),
            postal_code: "90210".to_string(),
            currency: "USD".to_string(),
            tax_zone_id: Uuid::now_v7(),
            timezone: "America/Los_Angeles".to_string(),
            is_warehouse: false,
        },
    )
    .await
    .unwrap();
    df_db::insert_product(
        pool,
        &Product {
            id: Uuid::from_u128(PRODUCT_ID),
            org_id: Uuid::from_u128(ORG_ID),
            sku: "BM-REGAL-1G".to_string(),
            manufacturer_code: Some("BM".to_string()),
            name: "Regal Select Interior Gallon".to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: "paint_base".to_string(),
            base_uom: "gal".to_string(),
            list_price: dec("100.0000"),
            standard_cost: dec("50.0000"),
            avg_cost: dec("50.0000"),
            min_margin_pct: None,
            sheen: Some("Eggshell".to_string()),
            paint_base_code: Some("1X".to_string()),
            container_volume_ml: Some(3785),
            eco_fee_category: Some("architectural_paint".to_string()),
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            upc: None,
            category: None,
            line_code: None,
            color_name: None,
            size_code: None,
            source_metadata: None,
            is_active: true,
            map: None,
            msrp: None,
        },
    )
    .await
    .unwrap();
    let org_id = Uuid::from_u128(ORG_ID);
    for (id, number, kind) in [
        (CUSTOMER_A, "C-A", "contractor"),
        (CUSTOMER_B, "C-B", "contractor"),
        (CUSTOMER_C, "C-C", "contractor"),
        (CUSTOMER_D, "C-D", "retail"),
    ] {
        df_db::insert_customer(pool, &customer(Uuid::from_u128(id), org_id, number, kind))
            .await
            .unwrap();
    }
    edge
}

fn cash_sale_for(customer_id: Uuid) -> SaleInput {
    SaleInput {
        customer_id: Some(customer_id),
        job_id: None,
        lines: vec![SaleLineInput {
            product_id: Uuid::from_u128(PRODUCT_ID),
            sku: "BM-REGAL-1G".to_string(),
            description: "Regal Select Interior Gallon".to_string(),
            qty: dec("1"),
            uom: "gal".to_string(),
            unit_price: money("100.0000"),
            unit_cost: money("50.0000"),
        }],
        tenders: vec![TenderInput {
            kind: TenderKind::Cash,
            amount: None,
            reference: None,
        }],
        po_number: None,
        job_name: None,
        price_level_id: None,
    }
}

/// Move a completed sale's timestamp back so the lapsed report
/// sees it as an old purchase. The sale itself went through the
/// real checkout; only the clock is adjusted for the test.
async fn backdate_sale(edge: &Edge, transaction_id: Uuid, days_ago: i64) {
    let at = Utc::now() - TimeDelta::days(days_ago);
    sqlx::query("UPDATE transactions SET created_at = $1 WHERE id = $2")
        .bind(at.to_rfc3339())
        .bind(transaction_id.to_string())
        .execute(edge.pool())
        .await
        .unwrap();
}

fn at_today(hour: u32) -> DateTime<Utc> {
    date(2026, 11, 26)
        .and_hms_opt(hour, 0, 0)
        .unwrap()
        .and_utc()
}

#[tokio::test]
async fn crm_lapsed_tickler_appointments_saved_views() {
    let edge = test_edge().await;
    let terminal = MockTerminal::new();
    let cashier = Uuid::from_u128(0x6000);
    edge.open_till(cashier, money("150.0000")).await.unwrap();

    // --- Purchases: A 100 days ago, C today, D (retail) 100 days ago. ---
    let sale_a = edge
        .checkout(cash_sale_for(Uuid::from_u128(CUSTOMER_A)), &terminal)
        .await
        .unwrap();
    backdate_sale(&edge, sale_a.transaction_id, 100).await;
    let _sale_c = edge
        .checkout(cash_sale_for(Uuid::from_u128(CUSTOMER_C)), &terminal)
        .await
        .unwrap();
    let sale_d = edge
        .checkout(cash_sale_for(Uuid::from_u128(CUSTOMER_D)), &terminal)
        .await
        .unwrap();
    backdate_sale(&edge, sale_d.transaction_id, 100).await;

    // --- Lapsed report: B (never) first, then A (lapsed). ---
    let report = edge
        .lapsed_contractors(Utc::now().date_naive(), 90, true)
        .await
        .unwrap();
    assert_eq!(report.customers.len(), 2);
    assert_eq!(report.customers[0].customer_id, Uuid::from_u128(CUSTOMER_B));
    assert_eq!(
        report.customers[0].status,
        df_crm::LapsedStatus::NeverPurchased
    );
    assert_eq!(
        report.customers[0].trailing_spend,
        Decimal::ZERO,
        "never purchased, no trailing spend"
    );
    assert_eq!(report.customers[1].customer_id, Uuid::from_u128(CUSTOMER_A));
    assert_eq!(report.customers[1].status, df_crm::LapsedStatus::Lapsed);
    assert!(
        report.customers[1].trailing_spend > Decimal::ZERO,
        "A's 100-day-old sale is inside the trailing 365 days"
    );
    assert_eq!(report.customers[1].ar_balance, Decimal::ZERO);
    // Without the never-purchased, only A remains; with a 200-day
    // window A is not lapsed either (100 < 200, strictly more).
    let report = edge
        .lapsed_contractors(Utc::now().date_naive(), 90, false)
        .await
        .unwrap();
    assert_eq!(report.customers.len(), 1);
    let report = edge
        .lapsed_contractors(Utc::now().date_naive(), 200, true)
        .await
        .unwrap();
    assert_eq!(report.customers.len(), 1);
    assert_eq!(
        report.customers[0].customer_id,
        Uuid::from_u128(CUSTOMER_B),
        "only the never-purchased contractor is left at 200 days"
    );

    // --- Tickler: due, complete, snooze, and the error cases. ---
    let now = Utc::now();
    let reminder = edge
        .create_activity(CreateActivityRequest {
            customer_id: Some(Uuid::from_u128(CUSTOMER_A)),
            activity_type: "follow_up".to_string(),
            subject: "Call A about the Miller job".to_string(),
            notes: None,
            created_by: None,
            due_at: Some(now - TimeDelta::hours(2)),
        })
        .await
        .unwrap();
    let due = edge.due_reminders(now).await.unwrap();
    assert!(due.iter().any(|a| a.id == reminder.id));
    let overdue = edge.overdue_reminders(now).await.unwrap();
    assert!(overdue.iter().any(|a| a.id == reminder.id));
    let done = edge.complete_activity(reminder.id).await.unwrap();
    assert!(done.completed_at.is_some());
    assert!(
        !edge
            .due_reminders(now)
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == reminder.id),
        "a completed reminder leaves the due list"
    );
    assert!(
        edge.complete_activity(reminder.id).await.is_err(),
        "completing an already-completed reminder is an error"
    );
    assert!(
        edge.snooze_activity(reminder.id, now + TimeDelta::hours(1))
            .await
            .is_err(),
        "a completed reminder cannot be snoozed"
    );

    let second = edge
        .create_activity(CreateActivityRequest {
            customer_id: Some(Uuid::from_u128(CUSTOMER_A)),
            activity_type: "follow_up".to_string(),
            subject: "Second follow-up".to_string(),
            notes: None,
            created_by: None,
            due_at: Some(now - TimeDelta::hours(1)),
        })
        .await
        .unwrap();
    assert!(
        edge.snooze_activity(second.id, now - TimeDelta::hours(1))
            .await
            .is_err(),
        "snoozing to the past is an error"
    );
    let snoozed = edge
        .snooze_activity(second.id, now + TimeDelta::hours(2))
        .await
        .unwrap();
    assert_eq!(snoozed.snoozed_until, Some(now + TimeDelta::hours(2)));
    assert!(
        !edge
            .due_reminders(now)
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == second.id),
        "a snoozed reminder stays hidden until its snooze passes"
    );

    // A note with no due date is never a reminder.
    let note = edge
        .create_activity(CreateActivityRequest {
            customer_id: None,
            activity_type: "note".to_string(),
            subject: "Just a note".to_string(),
            notes: None,
            created_by: None,
            due_at: None,
        })
        .await
        .unwrap();
    assert!(
        !edge
            .due_reminders(now)
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == note.id)
    );

    // --- Appointments: same contractor OR same staff blocks. ---
    let staff_s = Uuid::from_u128(STAFF_S);
    let staff_t = Uuid::from_u128(STAFF_T);
    let customer_a = Uuid::from_u128(CUSTOMER_A);
    let customer_b = Uuid::from_u128(CUSTOMER_B);
    let first = edge
        .create_appointment(CreateAppointmentRequest {
            customer_id: Some(customer_a),
            staff_id: Some(staff_s),
            title: "Measure — A kitchen".to_string(),
            starts_at: at_today(10),
            ends_at: at_today(11),
            notes: None,
        })
        .await
        .unwrap();
    assert_eq!(first.status, "scheduled");
    // Same contractor, different staff, overlapping: blocked.
    assert!(
        edge.create_appointment(CreateAppointmentRequest {
            customer_id: Some(customer_a),
            staff_id: Some(staff_t),
            title: "Clash — same contractor".to_string(),
            starts_at: at_today(10) + TimeDelta::minutes(30),
            ends_at: at_today(11) + TimeDelta::minutes(30),
            notes: None,
        })
        .await
        .is_err()
    );
    // Same staff, different contractor, overlapping: blocked.
    assert!(
        edge.create_appointment(CreateAppointmentRequest {
            customer_id: Some(customer_b),
            staff_id: Some(staff_s),
            title: "Clash — same staff".to_string(),
            starts_at: at_today(10) + TimeDelta::minutes(30),
            ends_at: at_today(11) + TimeDelta::minutes(30),
            notes: None,
        })
        .await
        .is_err()
    );
    // Adjacent to the first slot: free.
    let adjacent = edge
        .create_appointment(CreateAppointmentRequest {
            customer_id: Some(customer_a),
            staff_id: Some(staff_s),
            title: "Adjacent — free".to_string(),
            starts_at: at_today(11),
            ends_at: at_today(12),
            notes: None,
        })
        .await
        .unwrap();
    assert_eq!(adjacent.status, "scheduled");
    // Different contractor AND staff, same clock time: free.
    edge.create_appointment(CreateAppointmentRequest {
        customer_id: Some(customer_b),
        staff_id: Some(staff_t),
        title: "Parallel — free".to_string(),
        starts_at: at_today(10) + TimeDelta::minutes(30),
        ends_at: at_today(11) + TimeDelta::minutes(30),
        notes: None,
    })
    .await
    .unwrap();
    // A zero-length window is rejected at construction.
    assert!(
        edge.create_appointment(CreateAppointmentRequest {
            customer_id: None,
            staff_id: None,
            title: "Zero window".to_string(),
            starts_at: at_today(14),
            ends_at: at_today(14),
            notes: None,
        })
        .await
        .is_err()
    );
    // Cancelling frees the slot: the same contractor can rebook it.
    edge.cancel_appointment(first.id).await.unwrap();
    edge.create_appointment(CreateAppointmentRequest {
        customer_id: Some(customer_a),
        staff_id: Some(staff_s),
        title: "Rebook after cancel".to_string(),
        starts_at: at_today(10),
        ends_at: at_today(11),
        notes: None,
    })
    .await
    .unwrap();
    // By-day list: all of today's appointments (including the
    // cancelled one — the list does not filter by status).
    let today = edge
        .list_appointments(Some(date(2026, 11, 26)), None, None)
        .await
        .unwrap();
    assert_eq!(today.len(), 4, "adjacent + parallel + rebooked + cancelled");
    assert!(
        today
            .iter()
            .all(|a| a.starts_at.date_naive() == date(2026, 11, 26))
    );

    // --- Saved views: the two dialects stay distinct. ---
    let activity_view = edge
        .create_saved_view(CreateSavedViewRequest {
            name: "Open follow-ups".to_string(),
            filter_json: serde_json::json!({
                "activity_type": "follow_up",
                "completed": false,
            }),
        })
        .await
        .unwrap();
    let hits = edge.run_saved_view(activity_view.id).await.unwrap();
    assert_eq!(hits.len(), 1, "only the open follow-up (second)");
    assert_eq!(hits[0].id, second.id);
    // The customer dialect rejects the activity view explicitly.
    assert!(
        edge.run_saved_view_customers(activity_view.id)
            .await
            .is_err(),
        "an activity-dialect view must not be evaluated over customers"
    );

    let customer_view = edge
        .create_saved_view(CreateSavedViewRequest {
            name: "Contractors who bought".to_string(),
            filter_json: serde_json::json!({
                "customer_type": "contractor",
                "has_purchased": true,
            }),
        })
        .await
        .unwrap();
    let customers = edge
        .run_saved_view_customers(customer_view.id)
        .await
        .unwrap();
    let ids: Vec<Uuid> = customers.iter().map(|c| c.customer_id).collect();
    assert_eq!(ids.len(), 2, "A and C bought; B never did, D is retail");
    assert!(ids.contains(&customer_a));
    assert!(ids.contains(&Uuid::from_u128(CUSTOMER_C)));
    // The activity endpoint on a customer-dialect view parses
    // leniently (unknown keys ignored) and matches everything —
    // that is the documented behaviour; the customer endpoint is
    // the one that knows the dialect.
    let all = edge.run_saved_view(customer_view.id).await.unwrap();
    assert_eq!(all.len(), 3, "all activities: reminder + second + note");

    // --- Activity CRUD round-trip. ---
    let fetched = edge.get_activity(note.id).await.unwrap();
    assert_eq!(fetched.subject, "Just a note");
    let updated = edge
        .update_activity(
            note.id,
            df_edge::UpdateActivityRequest {
                subject: Some("Edited note".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.subject, "Edited note");
    edge.delete_activity(note.id).await.unwrap();
    assert!(edge.get_activity(note.id).await.is_err());
}
