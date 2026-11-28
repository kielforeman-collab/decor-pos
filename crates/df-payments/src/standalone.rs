//! Standalone (dumb) external terminal mode (spec §1).
//!
//! Some stores use a terminal that is not integrated with the POS at
//! all: the cashier reads the amount off the screen and keys it into the
//! physical terminal by hand. There is no terminal protocol here — this
//! provider only records a [`ReconciliationRecord`] per authorization so
//! the sale can be reconciled against the terminal's own batch report
//! at close-out. The reference is `standalone_<idempotency_key>`.
//!
//! When [`StandaloneConfig::require_manual_confirmation`] is set, an
//! authorization is recorded as *pending* and only moves into
//! [`StandaloneTerminal::records`] once the cashier calls
//! [`StandaloneTerminal::confirm`] with the reference (e.g. after seeing
//! the approval slip). Until then the returned [`PaymentResult`] is
//! provisional.

use crate::error::PaymentError;
use crate::provider::{PaymentProvider, PaymentRequest, PaymentResult, PaymentStatus, TenderKind};
use chrono::{DateTime, Utc};
use df_domain::Money;
use std::collections::HashMap;
use std::sync::Mutex;

/// Configuration for [`StandaloneTerminal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StandaloneConfig {
    /// When true, authorizations stay pending until [`StandaloneTerminal::confirm`]
    /// is called with the reference.
    pub require_manual_confirmation: bool,
}

/// One entry in the reconciliation log: what was keyed into the external
/// terminal, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationRecord {
    pub reference: String,
    pub amount: Money,
    pub authorized_at: DateTime<Utc>,
}

/// A [`PaymentProvider`] that models a non-integrated terminal. It never
/// talks to hardware or a network — it only records what the cashier is
/// expected to key in by hand.
pub struct StandaloneTerminal {
    config: StandaloneConfig,
    records: Mutex<Vec<ReconciliationRecord>>,
    pending: Mutex<HashMap<String, ReconciliationRecord>>,
    tenders: Mutex<HashMap<String, TenderKind>>,
}

impl StandaloneTerminal {
    /// No manual confirmation: authorizations are recorded immediately.
    pub fn new() -> Self {
        Self::with_config(StandaloneConfig::default())
    }

    pub fn with_config(config: StandaloneConfig) -> Self {
        Self {
            config,
            records: Mutex::new(Vec::new()),
            pending: Mutex::new(HashMap::new()),
            tenders: Mutex::new(HashMap::new()),
        }
    }

    /// Confirmed reconciliation records, in order.
    pub fn records(&self) -> Vec<ReconciliationRecord> {
        self.records.lock().expect("standalone lock").clone()
    }

    /// Authorizations awaiting manual confirmation.
    pub fn pending(&self) -> Vec<ReconciliationRecord> {
        self.pending
            .lock()
            .expect("standalone lock")
            .values()
            .cloned()
            .collect()
    }

    /// Confirm a pending authorization, moving it into [`Self::records`].
    /// Returns `true` when a pending entry with this reference existed.
    pub fn confirm(&self, reference: &str) -> bool {
        let record = self
            .pending
            .lock()
            .expect("standalone lock")
            .remove(reference);
        match record {
            Some(record) => {
                self.records.lock().expect("standalone lock").push(record);
                true
            }
            None => false,
        }
    }
}

impl Default for StandaloneTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl PaymentProvider for StandaloneTerminal {
    fn name(&self) -> &'static str {
        "standalone"
    }

    fn authorize<'a>(
        &'a self,
        req: &'a PaymentRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PaymentResult, PaymentError>> + Send + 'a>,
    > {
        let req = req.clone();
        Box::pin(async move {
            let reference = format!("standalone_{}", req.idempotency_key);
            let record = ReconciliationRecord {
                reference: reference.clone(),
                amount: req.amount,
                authorized_at: Utc::now(),
            };
            if self.config.require_manual_confirmation {
                self.pending
                    .lock()
                    .expect("standalone lock")
                    .insert(reference.clone(), record);
            } else {
                self.records.lock().expect("standalone lock").push(record);
            }
            self.tenders
                .lock()
                .expect("standalone lock")
                .insert(reference.clone(), req.tender);
            Ok(PaymentResult {
                status: PaymentStatus::Approved,
                // The physical terminal prints its own approval code on the
                // slip; this crate has no way to know it.
                authorization: None,
                reference,
                amount: req.amount,
                surcharge: req.surcharge,
                tender: req.tender,
            })
        })
    }

    fn void<'a>(
        &'a self,
        transaction_ref: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PaymentResult, PaymentError>> + Send + 'a>,
    > {
        let transaction_ref = transaction_ref.to_string();
        Box::pin(async move {
            // Remove from pending if unconfirmed, and record a void entry
            // so the end-of-day reconciliation log matches the physical terminal.
            let prior = self
                .pending
                .lock()
                .expect("standalone lock")
                .remove(&transaction_ref);
            let amount = prior
                .map(|r| r.amount)
                .or_else(|| {
                    self.records
                        .lock()
                        .expect("standalone lock")
                        .iter()
                        .find(|r| r.reference == transaction_ref)
                        .map(|r| r.amount)
                })
                .unwrap_or_else(|| Money::zero(df_domain::Currency::Usd));
            let reference = format!("void:{transaction_ref}");
            self.records
                .lock()
                .expect("standalone lock")
                .push(ReconciliationRecord {
                    reference: reference.clone(),
                    amount,
                    authorized_at: Utc::now(),
                });
            let tender = self
                .tenders
                .lock()
                .expect("standalone lock")
                .get(&transaction_ref)
                .copied()
                .unwrap_or(TenderKind::Card);
            Ok(PaymentResult {
                status: PaymentStatus::Voided,
                authorization: None,
                reference,
                amount,
                surcharge: Money::zero(amount.currency()),
                tender,
            })
        })
    }

    fn refund<'a>(
        &'a self,
        transaction_ref: &'a str,
        amount: Money,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PaymentResult, PaymentError>> + Send + 'a>,
    > {
        let transaction_ref = transaction_ref.to_string();
        Box::pin(async move {
            // A refund is a back-office action on the physical terminal; it is
            // recorded immediately even when authorizations need confirmation.
            let reference = format!("refund:{transaction_ref}");
            let record = ReconciliationRecord {
                reference: reference.clone(),
                amount,
                authorized_at: Utc::now(),
            };
            self.records.lock().expect("standalone lock").push(record);
            let tender = self
                .tenders
                .lock()
                .expect("standalone lock")
                .get(&transaction_ref)
                .copied()
                .unwrap_or(TenderKind::Card);
            Ok(PaymentResult {
                status: PaymentStatus::Approved,
                authorization: None,
                reference,
                amount,
                surcharge: Money::zero(amount.currency()),
                tender,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::Currency;
    use uuid::Uuid;

    fn usd(s: &str) -> Money {
        Money::from_str_amount(s, Currency::Usd).unwrap()
    }

    fn request(amount: &str) -> PaymentRequest {
        PaymentRequest {
            amount: usd(amount),
            idempotency_key: Uuid::now_v7(),
            tender: TenderKind::Card,
            surcharge: usd("0.0000"),
        }
    }

    #[tokio::test]
    async fn authorize_records_reconciliation_entry() {
        let terminal = StandaloneTerminal::new();
        assert_eq!(terminal.name(), "standalone");
        let req = request("60.0000");
        let result = terminal.authorize(&req).await.unwrap();
        assert_eq!(result.status, PaymentStatus::Approved);
        assert_eq!(
            result.reference,
            format!("standalone_{}", req.idempotency_key)
        );
        assert_eq!(result.authorization, None);
        let records = terminal.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].reference, result.reference);
        assert_eq!(records[0].amount, usd("60.0000"));
        assert!(terminal.pending().is_empty());
    }

    #[tokio::test]
    async fn refund_records_entry_and_keeps_tender() {
        let terminal = StandaloneTerminal::new();
        let sale = terminal.authorize(&request("50.0000")).await.unwrap();
        let refund = terminal
            .refund(&sale.reference, usd("10.0000"))
            .await
            .unwrap();
        assert_eq!(refund.status, PaymentStatus::Approved);
        assert_eq!(refund.tender, TenderKind::Card);
        assert_eq!(terminal.records().len(), 2);
        assert_eq!(terminal.records()[1].reference, refund.reference);
        assert_eq!(terminal.records()[1].amount, usd("10.0000"));
    }

    #[tokio::test]
    async fn confirmation_flow_holds_records_until_confirmed() {
        let terminal = StandaloneTerminal::with_config(StandaloneConfig {
            require_manual_confirmation: true,
        });
        let req = request("33.0000");
        let result = terminal.authorize(&req).await.unwrap();
        // Provisional result returned, but nothing is reconciled yet.
        assert!(terminal.records().is_empty());
        assert_eq!(terminal.pending().len(), 1);
        assert_eq!(terminal.pending()[0].reference, result.reference);

        assert!(!terminal.confirm("standalone_nope"));
        assert!(terminal.records().is_empty());

        assert!(terminal.confirm(&result.reference));
        assert_eq!(terminal.records().len(), 1);
        assert!(terminal.pending().is_empty());
        // Confirming twice is a no-op.
        assert!(!terminal.confirm(&result.reference));
    }
}
