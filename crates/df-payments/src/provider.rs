//! The [`PaymentProvider`] trait and its request/result types (spec §6.2).
//!
//! Providers are semi-integrated / tokenized flows only. A
//! [`PaymentResult::reference`] is a provider reference or token (e.g.
//! `mock_1`, `standalone_<uuid>`); [`PaymentResult::authorization`] is
//! an approval-code string. Neither ever carries raw card data — there
//! is no field for it anywhere in this crate.

use crate::error::PaymentError;
use df_domain::Money;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How the customer is paying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TenderKind {
    Cash,
    Card,
    ChargeAccount,
    GiftCard,
}

impl TenderKind {
    /// The `payments.tender` column value for this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            TenderKind::Cash => "cash",
            TenderKind::Card => "card",
            TenderKind::ChargeAccount => "charge_account",
            TenderKind::GiftCard => "gift_card",
        }
    }
}

/// Outcome of an authorization or refund.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaymentStatus {
    Approved,
    Declined,
    Voided,
}

/// One authorization attempt. `amount` is the total to collect on
/// this tender (including any surcharge); `surcharge` is the surcharge
/// portion of it, already computed by
/// [`crate::surcharge::compute_surcharge`] (zero for tenders with no
/// surcharge rule).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentRequest {
    pub amount: Money,
    pub idempotency_key: Uuid,
    pub tender: TenderKind,
    pub surcharge: Money,
}

/// Outcome of an authorization or refund. `reference` is the provider's
/// reference/token for this transaction; `authorization` is the approval
/// code when the provider supplies one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentResult {
    pub status: PaymentStatus,
    pub authorization: Option<String>,
    pub reference: String,
    pub amount: Money,
    pub surcharge: Money,
    pub tender: TenderKind,
}

/// A payment terminal / provider. Implemented by [`crate::MockTerminal`]
/// (tests), [`crate::StandaloneTerminal`] (dumb external terminal), and
/// [`crate::StripeTerminal`] (live calls land in a later phase).
///
/// A literal `async fn` in a trait is not dyn-compatible in Rust 1.99
/// (E0038), and neither is the `-> impl Future` desugaring, so the
/// trait uses the equivalent boxed-future form below: no `async_trait`
/// macro, callers still write `provider.authorize(&req).await`, and
/// implementors wrap their async block in `Box::pin` (see
/// [`crate::MockTerminal`]). `&dyn PaymentProvider` works everywhere.
pub trait PaymentProvider: Send + Sync {
    /// Stable provider name, e.g. `"mock"`, `"standalone"`,
    /// `"stripe_terminal"`.
    fn name(&self) -> &'static str;

    /// Authorize (and capture) `req.amount` (including `req.surcharge`).
    /// Providers must be idempotent on `req.idempotency_key`: replaying
    /// the same key returns the original result.
    fn authorize<'a>(
        &'a self,
        req: &'a PaymentRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PaymentResult, PaymentError>> + Send + 'a>,
    >;

    /// Refund `amount` against the transaction identified by
    /// `transaction_ref` (a [`PaymentResult::reference`]).
    fn refund<'a>(
        &'a self,
        transaction_ref: &'a str,
        amount: Money,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PaymentResult, PaymentError>> + Send + 'a>,
    >;
}
