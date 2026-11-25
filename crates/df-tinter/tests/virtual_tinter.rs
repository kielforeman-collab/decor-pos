//! Integration tests for the Phase 2 tinting half (spec §7 exit:
//! dispense → costed invoice → can label → formula reorder).
//!
//! Covers the full loop through the public API only: seed catalog +
//! colorants → formula lookup → virtual dispense → job trace → can
//! label → customer history reorder.

use df_domain::tint::tint_cost;
use df_domain::{Currency, Money};
use df_tinter::{DispenseRequest, DispensedShot};
use df_tinter::{
    FormulaHistory, StoredFormula, TintCanLabel, TinterError, TinterPlugin, VirtualTinter,
    search_colors, seed_catalog, seed_colorants, tint_can_label_zpl,
};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[test]
fn end_to_end_dispense_cost_label_and_reorder() {
    // 1. Look up the color (case-insensitive, manufacturer-scoped).
    let catalog = seed_catalog();
    let found = search_colors(&catalog, "Benjamin Moore", "white dove");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "OC-17");

    // 2. Build the formula from the seed colorants and cost it.
    let seeds = seed_colorants();
    let y3 = seeds.iter().find(|c| c.code == "Y3").unwrap();
    let lines = vec![y3.to_tint_colorant(dec("0"), 14)];
    assert_eq!(
        tint_cost(&lines).unwrap(),
        Money::from_str_amount("0.1225", Currency::Usd).unwrap()
    );

    // 3. Dispense it on the virtual tinter.
    let mut tinter = VirtualTinter::new("virtual-simulator");
    tinter.set_level("Y3", dec("10"), dec("2"));
    let customer_id = Uuid::now_v7();
    let formula_id = Uuid::now_v7();
    let transaction_id = Uuid::now_v7();
    let ack = tinter
        .send_formula(DispenseRequest {
            formula_id,
            base_product_id: Uuid::now_v7(),
            base_product_name: "Regal Select Interior Gallon".to_string(),
            container_volume_ml: 3785,
            colorants: lines.clone(),
            transaction_id: Some(transaction_id),
            line_no: Some(1),
            dispenser: None,
        })
        .unwrap();
    assert_eq!(
        tinter.level("Y3").unwrap().on_hand_fl_oz,
        dec("10") - dec("14") / dec("48")
    );

    // 4. The tinted sale line traces to its dispense record.
    let job = tinter.job(ack.job_id).unwrap();
    assert_eq!(job.formula_id, formula_id);
    assert_eq!(job.transaction_id, Some(transaction_id));
    assert_eq!(job.line_no, Some(1));
    assert_eq!(job.dispensed[0].code, "Y3");

    // 5. Print the can label from the dispensed amounts.
    let zpl = tint_can_label_zpl(&TintCanLabel {
        product_name: "Regal Select Interior Gallon".to_string(),
        color_code: "OC-17".to_string(),
        color_name: "White Dove".to_string(),
        sheen: "Eggshell".to_string(),
        base: "1X".to_string(),
        formula_ref: format!("F-{formula_id}"),
        barcode: "012345678905".to_string(),
        shots: job.dispensed.clone(),
    });
    assert!(zpl.contains("OC-17 White Dove"));
    assert!(zpl.contains("Y3: 0.2916666666666666666666666667 fl oz"));

    // 6. Record the formula in the customer's history; a repeat tint
    // pulls the same formula.
    let mut history = FormulaHistory::new();
    history.record(StoredFormula {
        id: formula_id,
        customer_id: Some(customer_id),
        job_id: None,
        color_code: "OC-17".to_string(),
        color_name: "White Dove".to_string(),
        base_product_id: Uuid::now_v7(),
        dispensed_at: chrono::Utc::now(),
        colorants: lines,
    });
    let past = history.history_for_customer(customer_id, None);
    assert_eq!(past.len(), 1);
    assert_eq!(past[0].color_code, "OC-17");
}

#[test]
fn shortage_blocks_the_sale_and_surfaces_an_alert() {
    let mut tinter = VirtualTinter::new("virtual-simulator");
    tinter.set_level("Y3", dec("0.1"), dec("2"));
    // Already at/below threshold before dispensing.
    assert_eq!(tinter.shortage_alerts().len(), 1);
    assert_eq!(tinter.shortage_alerts()[0].code, "Y3");

    let seeds = seed_colorants();
    let y3 = seeds.iter().find(|c| c.code == "Y3").unwrap();
    let err = tinter
        .send_formula(DispenseRequest {
            formula_id: Uuid::now_v7(),
            base_product_id: Uuid::now_v7(),
            base_product_name: "Regal Select Interior Gallon".to_string(),
            container_volume_ml: 3785,
            colorants: vec![y3.to_tint_colorant(dec("0"), 14)],
            transaction_id: None,
            line_no: None,
            dispenser: None,
        })
        .unwrap_err();
    assert!(matches!(err, TinterError::InsufficientColorant { .. }));
    assert_eq!(tinter.level("Y3").unwrap().on_hand_fl_oz, dec("0.1"));
    let job = &tinter.jobs()[0];
    assert_eq!(job.status, df_tinter::TintJobStatus::Failed);
    let _ = DispensedShot {
        code: "Y3".to_string(),
        fl_oz: dec("0"),
    };
}
