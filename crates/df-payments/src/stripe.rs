//! Stripe Terminal provider — trait-implementation skeleton (Phase 1).
//!
//! Live Stripe Terminal calls are a later phase. This provider exists so
//! `df-edge` can depend on the [`PaymentProvider`] trait with the real
//! provider shape today:
//!
//! * Credentials come **only** from the environment at construction
//!   ([`StripeTerminal::from_env`], reading `STRIPE_API_KEY` and
//!   `STRIPE_TERMINAL_LOCATION_ID`). They are held in the struct fields
//!   and nowhere else — never logged, never written to disk.
//! * `authorize` / `refund` return [`PaymentError::MissingCredentials`]
//!   when constructed without credentials, and
//!   [`PaymentError::NotSupported`] when credentials are present —
//!   **no HTTP call is ever made in this phase**.
//!
//! No `Debug` impl on purpose: deriving it would print the API key.

use crate::error::PaymentError;
use crate::provider::{PaymentProvider, PaymentRequest, PaymentResult};
use df_domain::Money;

/// Stripe Terminal provider (skeleton — live calls not implemented).
pub struct StripeTerminal {
    api_key: String,
    location_id: String,
}

impl StripeTerminal {
    /// Build from explicit credentials (tests use obviously fake values
    /// such as `"sk_test_fake"`). The key is stored in the struct field
    /// only.
    pub fn new(api_key: String, location_id: String) -> Self {
        Self {
            api_key,
            location_id,
        }
    }

    /// Build from the environment. Errors with
    /// [`PaymentError::MissingCredentials`] unless both
    /// `STRIPE_API_KEY` and `STRIPE_TERMINAL_LOCATION_ID` are set and
    /// non-empty.
    pub fn from_env() -> Result<Self, PaymentError> {
        let api_key = std::env::var("STRIPE_API_KEY").unwrap_or_default();
        let location_id = std::env::var("STRIPE_TERMINAL_LOCATION_ID").unwrap_or_default();
        if api_key.trim().is_empty() || location_id.trim().is_empty() {
            return Err(PaymentError::MissingCredentials(
                "STRIPE_API_KEY and STRIPE_TERMINAL_LOCATION_ID must both be set".to_string(),
            ));
        }
        Ok(Self::new(api_key, location_id))
    }

    fn has_credentials(&self) -> bool {
        !self.api_key.trim().is_empty() && !self.location_id.trim().is_empty()
    }
}

impl PaymentProvider for StripeTerminal {
    fn name(&self) -> &'static str {
        "stripe_terminal"
    }

    fn authorize<'a>(
        &'a self,
        _req: &'a PaymentRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PaymentResult, PaymentError>> + Send + 'a>,
    > {
        let has_credentials = self.has_credentials();
        Box::pin(async move {
            if !has_credentials {
                return Err(PaymentError::MissingCredentials(
                    "Stripe Terminal credentials are not configured".to_string(),
                ));
            }
            Err(PaymentError::NotSupported(
                "Stripe Terminal live calls are not implemented in Phase 1".to_string(),
            ))
        })
    }

    fn refund<'a>(
        &'a self,
        _transaction_ref: &'a str,
        _amount: Money,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<PaymentResult, PaymentError>> + Send + 'a>,
    > {
        let has_credentials = self.has_credentials();
        Box::pin(async move {
            if !has_credentials {
                return Err(PaymentError::MissingCredentials(
                    "Stripe Terminal credentials are not configured".to_string(),
                ));
            }
            Err(PaymentError::NotSupported(
                "Stripe Terminal live calls are not implemented in Phase 1".to_string(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TenderKind;
    use df_domain::Currency;
    use uuid::Uuid;

    fn request() -> PaymentRequest {
        PaymentRequest {
            amount: Money::from_str_amount("10.0000", Currency::Usd).unwrap(),
            idempotency_key: Uuid::now_v7(),
            tender: TenderKind::Card,
            surcharge: Money::zero(Currency::Usd),
        }
    }

    #[tokio::test]
    async fn empty_credentials_are_missing_credentials() {
        let terminal = StripeTerminal::new(String::new(), String::new());
        assert_eq!(
            terminal.authorize(&request()).await,
            Err(PaymentError::MissingCredentials(
                "Stripe Terminal credentials are not configured".to_string()
            ))
        );
        assert_eq!(
            terminal
                .refund("txn_1", Money::from_units(5, Currency::Usd))
                .await,
            Err(PaymentError::MissingCredentials(
                "Stripe Terminal credentials are not configured".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn fake_credentials_are_not_supported_not_missing() {
        // Obviously fake values: no live call is attempted, and no network
        // access happens — the provider errors before any HTTP.
        let terminal =
            StripeTerminal::new("sk_test_fake".to_string(), "tml_fake_location".to_string());
        assert_eq!(
            terminal.authorize(&request()).await,
            Err(PaymentError::NotSupported(
                "Stripe Terminal live calls are not implemented in Phase 1".to_string()
            ))
        );
        assert_eq!(
            terminal
                .refund("txn_1", Money::from_units(5, Currency::Usd))
                .await,
            Err(PaymentError::NotSupported(
                "Stripe Terminal live calls are not implemented in Phase 1".to_string()
            ))
        );
    }

    /// One test owns the environment variables end-to-end (set → assert →
    /// unset) so parallel test threads cannot race each other on them.
    #[tokio::test]
    async fn from_env_errors_without_vars_and_reads_fake_vars() {
        // SAFETY: this test is the only one in the crate that touches
        // these variables, and it restores them before returning.
        unsafe {
            std::env::remove_var("STRIPE_API_KEY");
            std::env::remove_var("STRIPE_TERMINAL_LOCATION_ID");
        }
        assert!(matches!(
            StripeTerminal::from_env(),
            Err(PaymentError::MissingCredentials(_))
        ));

        unsafe {
            std::env::set_var("STRIPE_API_KEY", "sk_test_fake");
            std::env::set_var("STRIPE_TERMINAL_LOCATION_ID", "tml_fake_location");
        }
        let terminal = StripeTerminal::from_env().expect("fake env vars should parse");
        assert_eq!(terminal.name(), "stripe_terminal");
        // Credentials present: NotSupported, not MissingCredentials, and
        // still no network call.
        assert_eq!(
            terminal.authorize(&request()).await,
            Err(PaymentError::NotSupported(
                "Stripe Terminal live calls are not implemented in Phase 1".to_string()
            ))
        );

        unsafe {
            std::env::remove_var("STRIPE_API_KEY");
            std::env::remove_var("STRIPE_TERMINAL_LOCATION_ID");
        }
        assert!(matches!(
            StripeTerminal::from_env(),
            Err(PaymentError::MissingCredentials(_))
        ));
    }
}
