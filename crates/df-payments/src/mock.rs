//! Mock payment terminal for tests and offline development.
//!
//! Approves everything by default; [`MockTerminal::declining`] declines
//! everything; [`MockTerminal::with_script`] consumes a scripted
//! [`MockOutcome`] per `authorize` call (an exhausted script approves).
//! Every request is recorded ([`MockTerminal::calls`]) and approvals are
//! idempotent on the request's idempotency key: replaying a key returns
//! the stored result without consuming another scripted outcome or
//! recording a second call. No network, no hardware.

use crate::error::PaymentError;
use crate::provider::{PaymentProvider, PaymentRequest, PaymentResult, PaymentStatus, TenderKind};
use df_domain::Money;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use uuid::Uuid;

/// One scripted outcome for [`MockTerminal::with_script`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockOutcome {
    Approve,
    Decline(String),
}

enum MockMode {
    ApproveAll,
    DeclineAll(String),
    Script(VecDeque<MockOutcome>),
}

/// In-memory [`PaymentProvider`] for tests. References look like
/// `mock_1`, `mock_2`, …; approval codes like `MOCK000001`.
pub struct MockTerminal {
    mode: Mutex<MockMode>,
    calls: Mutex<Vec<PaymentRequest>>,
    voided_refs: Mutex<Vec<String>>,
    refunded: Mutex<Vec<(String, Money)>>,
    approved_by_key: Mutex<HashMap<Uuid, PaymentResult>>,
    approved_by_ref: Mutex<HashMap<String, PaymentResult>>,
    counter: Mutex<u64>,
}

impl MockTerminal {
    /// Approves every authorization.
    pub fn new() -> Self {
        Self {
            mode: Mutex::new(MockMode::ApproveAll),
            calls: Mutex::new(Vec::new()),
            voided_refs: Mutex::new(Vec::new()),
            refunded: Mutex::new(Vec::new()),
            approved_by_key: Mutex::new(HashMap::new()),
            approved_by_ref: Mutex::new(HashMap::new()),
            counter: Mutex::new(0),
        }
    }

    /// Declines every authorization with `reason`.
    pub fn declining(reason: &str) -> Self {
        Self {
            mode: Mutex::new(MockMode::DeclineAll(reason.to_string())),
            calls: Mutex::new(Vec::new()),
            voided_refs: Mutex::new(Vec::new()),
            refunded: Mutex::new(Vec::new()),
            approved_by_key: Mutex::new(HashMap::new()),
            approved_by_ref: Mutex::new(HashMap::new()),
            counter: Mutex::new(0),
        }
    }

    /// Consumes one [`MockOutcome`] per `authorize` call, in order; once
    /// the script is exhausted, further calls approve.
    pub fn with_script(script: Vec<MockOutcome>) -> Self {
        Self {
            mode: Mutex::new(MockMode::Script(script.into())),
            calls: Mutex::new(Vec::new()),
            voided_refs: Mutex::new(Vec::new()),
            refunded: Mutex::new(Vec::new()),
            approved_by_key: Mutex::new(HashMap::new()),
            approved_by_ref: Mutex::new(HashMap::new()),
            counter: Mutex::new(0),
        }
    }

    /// Snapshot of every `authorize` request received, in order.
    /// Idempotent replays are not recorded twice.
    pub fn calls(&self) -> Vec<PaymentRequest> {
        self.calls.lock().expect("mock terminal lock").clone()
    }

    /// Snapshot of every `void` transaction reference received, in order.
    pub fn voided_refs(&self) -> Vec<String> {
        self.voided_refs.lock().expect("mock terminal lock").clone()
    }

    /// Snapshot of every `(transaction_ref, amount)` refund received, in order.
    pub fn refunds(&self) -> Vec<(String, Money)> {
        self.refunded.lock().expect("mock terminal lock").clone()
    }
}

impl Default for MockTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl PaymentProvider for MockTerminal {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn authorize<'a>(
        &'a self,
        req: &'a PaymentRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PaymentResult, PaymentError>> + Send + 'a>,
    > {
        Box::pin(async move {
            // Idempotent replay: same key returns the stored approval.
            if let Some(stored) = self
                .approved_by_key
                .lock()
                .expect("mock terminal lock")
                .get(&req.idempotency_key)
            {
                return Ok(stored.clone());
            }
            self.calls
                .lock()
                .expect("mock terminal lock")
                .push(req.clone());

            let outcome = {
                let mut mode = self.mode.lock().expect("mock terminal lock");
                match &mut *mode {
                    MockMode::ApproveAll => MockOutcome::Approve,
                    MockMode::DeclineAll(reason) => MockOutcome::Decline(reason.clone()),
                    MockMode::Script(queue) => queue.pop_front().unwrap_or(MockOutcome::Approve),
                }
            };
            match outcome {
                MockOutcome::Decline(reason) => Err(PaymentError::Declined(reason)),
                MockOutcome::Approve => {
                    let n = {
                        let mut counter = self.counter.lock().expect("mock terminal lock");
                        *counter += 1;
                        *counter
                    };
                    let result = PaymentResult {
                        status: PaymentStatus::Approved,
                        authorization: Some(format!("MOCK{n:06}")),
                        reference: format!("mock_{n}"),
                        amount: req.amount,
                        surcharge: req.surcharge,
                        tender: req.tender,
                    };
                    self.approved_by_key
                        .lock()
                        .expect("mock terminal lock")
                        .insert(req.idempotency_key, result.clone());
                    self.approved_by_ref
                        .lock()
                        .expect("mock terminal lock")
                        .insert(result.reference.clone(), result.clone());
                    Ok(result)
                }
            }
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
            self.voided_refs
                .lock()
                .expect("mock terminal lock")
                .push(transaction_ref.clone());
            let prior = self
                .approved_by_ref
                .lock()
                .expect("mock terminal lock")
                .get(&transaction_ref)
                .cloned();
            let n = {
                let mut counter = self.counter.lock().expect("mock terminal lock");
                *counter += 1;
                *counter
            };
            let (amount, surcharge, tender) = match prior {
                Some(res) => (res.amount, res.surcharge, res.tender),
                None => (
                    Money::zero(df_domain::Currency::Usd),
                    Money::zero(df_domain::Currency::Usd),
                    TenderKind::Card,
                ),
            };
            Ok(PaymentResult {
                status: PaymentStatus::Voided,
                authorization: Some(format!("MOCKVD{n:06}")),
                reference: format!("mock_void_{n}"),
                amount,
                surcharge,
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
            self.refunded
                .lock()
                .expect("mock terminal lock")
                .push((transaction_ref.clone(), amount));
            // Refunds keep the original tender when the transaction is known.
            let tender = self
                .approved_by_ref
                .lock()
                .expect("mock terminal lock")
                .get(&transaction_ref)
                .map(|result| result.tender)
                .unwrap_or(TenderKind::Card);
            let n = {
                let mut counter = self.counter.lock().expect("mock terminal lock");
                *counter += 1;
                *counter
            };
            Ok(PaymentResult {
                status: PaymentStatus::Approved,
                authorization: Some(format!("MOCKRF{n:06}")),
                reference: format!("mock_refund_{n}"),
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

    fn usd(s: &str) -> Money {
        Money::from_str_amount(s, Currency::Usd).unwrap()
    }

    fn request(amount: &str, tender: TenderKind) -> PaymentRequest {
        PaymentRequest {
            amount: usd(amount),
            idempotency_key: Uuid::now_v7(),
            tender,
            surcharge: usd("0.0000"),
        }
    }

    #[tokio::test]
    async fn approves_everything_by_default() {
        let terminal = MockTerminal::new();
        assert_eq!(terminal.name(), "mock");
        let req = request("42.0000", TenderKind::Card);
        let result = terminal.authorize(&req).await.unwrap();
        assert_eq!(result.status, PaymentStatus::Approved);
        assert_eq!(result.reference, "mock_1");
        assert_eq!(result.authorization, Some("MOCK000001".to_string()));
        assert_eq!(result.amount, usd("42.0000"));
        assert_eq!(result.tender, TenderKind::Card);
        assert_eq!(terminal.calls(), vec![req]);
    }

    #[tokio::test]
    async fn declining_terminal_declines_everything() {
        let terminal = MockTerminal::declining("insufficient funds");
        let req = request("10.0000", TenderKind::Card);
        assert_eq!(
            terminal.authorize(&req).await,
            Err(PaymentError::Declined("insufficient funds".to_string()))
        );
        // The attempt is still recorded.
        assert_eq!(terminal.calls(), vec![req]);
    }

    #[tokio::test]
    async fn script_is_consumed_in_order_then_approves() {
        let terminal = MockTerminal::with_script(vec![
            MockOutcome::Approve,
            MockOutcome::Decline("do not honor".to_string()),
        ]);
        let first = terminal
            .authorize(&request("1.0000", TenderKind::Card))
            .await
            .unwrap();
        assert_eq!(first.status, PaymentStatus::Approved);
        assert_eq!(
            terminal
                .authorize(&request("1.0000", TenderKind::Card))
                .await,
            Err(PaymentError::Declined("do not honor".to_string()))
        );
        // Script exhausted: approves again.
        let third = terminal
            .authorize(&request("1.0000", TenderKind::Card))
            .await
            .unwrap();
        assert_eq!(third.status, PaymentStatus::Approved);
        assert_eq!(terminal.calls().len(), 3);
    }

    #[tokio::test]
    async fn idempotent_replay_returns_same_result_without_new_call() {
        let terminal = MockTerminal::with_script(vec![
            MockOutcome::Approve,
            MockOutcome::Decline("would consume script if replayed".to_string()),
        ]);
        let req = request("25.0000", TenderKind::Card);
        let first = terminal.authorize(&req).await.unwrap();
        let replay = terminal.authorize(&req).await.unwrap();
        assert_eq!(replay, first);
        assert_eq!(replay.reference, "mock_1");
        assert_eq!(terminal.calls().len(), 1);
        // The scripted decline is still untouched: the next fresh key
        // consumes it.
        assert_eq!(
            terminal
                .authorize(&request("1.0000", TenderKind::Card))
                .await,
            Err(PaymentError::Declined(
                "would consume script if replayed".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn refund_keeps_original_tender() {
        let terminal = MockTerminal::new();
        let sale = terminal
            .authorize(&request("80.0000", TenderKind::GiftCard))
            .await
            .unwrap();
        let refund = terminal
            .refund(&sale.reference, usd("20.0000"))
            .await
            .unwrap();
        assert_eq!(refund.status, PaymentStatus::Approved);
        assert_eq!(refund.amount, usd("20.0000"));
        assert_eq!(refund.tender, TenderKind::GiftCard);
        assert!(refund.reference.starts_with("mock_refund_"));
    }
}
