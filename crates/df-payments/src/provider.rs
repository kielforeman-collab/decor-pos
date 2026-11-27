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

/// How the customer is paying. The card brands are distinct tender
/// kinds: a Visa sale authorizes through the card provider and
/// records `visa` (not generic `card`), so the till, the invoice
/// and the provider reconciliation all show the brand the customer
/// actually used. Serialized PascalCase (`"Visa"`); stored
/// lowercase (`visa`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TenderKind {
    Cash,
    Card,
    Visa,
    Mastercard,
    Amex,
    Debit,
    ETransfer,
    ChargeAccount,
    GiftCard,
}

impl TenderKind {
    /// The `payments.tender` column value for this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            TenderKind::Cash => "cash",
            TenderKind::Card => "card",
            TenderKind::Visa => "visa",
            TenderKind::Mastercard => "mastercard",
            TenderKind::Amex => "amex",
            TenderKind::Debit => "debit",
            TenderKind::ETransfer => "etransfer",
            TenderKind::ChargeAccount => "charge_account",
            TenderKind::GiftCard => "gift_card",
        }
    }

    /// True for the branded card kinds (Visa / Mastercard / Amex /
    /// Debit): they authorize as cards through the provider, with
    /// the same surcharge rule as generic `Card`.
    pub fn is_card_brand(self) -> bool {
        matches!(
            self,
            TenderKind::Visa | TenderKind::Mastercard | TenderKind::Amex | TenderKind::Debit
        )
    }

    /// True for generic `Card` and for the branded card kinds.
    pub fn is_card(self) -> bool {
        self == TenderKind::Card || self.is_card_brand()
    }

    /// The receipt / invoice label for this kind.
    pub fn label(self) -> &'static str {
        match self {
            TenderKind::Cash => "Cash",
            TenderKind::Card => "Card",
            TenderKind::Visa => "Visa",
            TenderKind::Mastercard => "Mastercard",
            TenderKind::Amex => "Amex",
            TenderKind::Debit => "Debit",
            TenderKind::ETransfer => "E-Transfer",
            TenderKind::ChargeAccount => "Charge Account",
            TenderKind::GiftCard => "Gift Card",
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

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::Currency;

    /// PCI guardrail (spec §8 guardrail 3): the payment types have no
    /// field that could carry card data, so no serialization of a
    /// request or result can ever leak a PAN/CVV. This pins the exact
    /// JSON key sets — adding a card field fails this test first.
    #[test]
    fn payment_types_carry_no_card_data() {
        let req = PaymentRequest {
            amount: Money::from_str_amount("42.0000", Currency::Usd).unwrap(),
            idempotency_key: Uuid::now_v7(),
            tender: TenderKind::Card,
            surcharge: Money::zero(Currency::Usd),
        };
        let req_json = serde_json::to_value(&req).unwrap();
        let req_keys: std::collections::BTreeSet<&str> = req_json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            req_keys,
            ["amount", "idempotency_key", "surcharge", "tender"]
                .into_iter()
                .collect()
        );

        let result = PaymentResult {
            status: PaymentStatus::Approved,
            authorization: Some("MOCK000001".to_string()),
            reference: "mock_1".to_string(),
            amount: req.amount,
            surcharge: req.surcharge,
            tender: TenderKind::Card,
        };
        let result_json = serde_json::to_value(&result).unwrap();
        let result_keys: std::collections::BTreeSet<&str> = result_json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            result_keys,
            [
                "amount",
                "authorization",
                "reference",
                "status",
                "surcharge",
                "tender"
            ]
            .into_iter()
            .collect()
        );

        // Belt and braces: no key anywhere mentions card data.
        let joined = format!("{req_json}{result_json}").to_lowercase();
        for forbidden in ["pan", "cvv", "cvc", "card_number", "cardnumber"] {
            assert!(
                !joined.contains(forbidden),
                "payment JSON contains `{forbidden}`"
            );
        }
    }
}
