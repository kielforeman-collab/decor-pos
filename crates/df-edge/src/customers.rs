//! Customer setup for the counter (`POST /pos/customers`,
//! `POST /pos/customers/{id}`): create and edit customer accounts.
//!
//! * [`Edge::create_customer`] — validates and inserts a customer.
//!   A blank account number is auto-assigned (`C-1001`, `C-1002`, …
//!   — one past the highest numeric `C-` account in this org).
//! * [`Edge::update_customer`] — replaces every editable field.
//!
//! Credit controls (charge account on/off, credit limit, terms,
//! finance rate, credit hold, portal access) are receivables
//! decisions: the HTTP layer passes `allow_credit = true` only for
//! roles holding `ArPayment` (Manager / Admin). A cashier can still
//! set up and edit contact, address, tax-exempt and price-level
//! details; a cashier request that tries to change a credit control
//! is rejected with [`EdgeError::Forbidden`] (403).
//!
//! All money is `Decimal` — never floats.

use chrono::NaiveDate;
use df_db::models::Customer;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

/// Customer types the setup form accepts.
pub const CUSTOMER_TYPES: &[&str] = &["retail", "contractor", "commercial"];

/// Default payment terms for a new customer.
pub const DEFAULT_PAYMENT_TERMS: &str = "NET30";

/// Body of `POST /pos/customers` (create) and `POST
/// /pos/customers/{id}` (full update). Every field is optional on
/// the wire; omitted fields take the defaults below. On update the
/// request **replaces** the stored values, so the form sends the
/// whole record (blank strings clear a field).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomerInput {
    /// Blank → auto-assigned on create; on update blank keeps the
    /// stored account number.
    pub account_number: Option<String>,
    /// `retail` (default), `contractor` or `commercial`.
    pub customer_type: Option<String>,
    pub company_name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    pub city: Option<String>,
    pub province: Option<String>,
    pub postal_code: Option<String>,
    pub notes: Option<String>,
    pub price_level_id: Option<Uuid>,
    pub tax_exempt_cert: Option<String>,
    pub tax_exempt_expires: Option<NaiveDate>,
    pub require_po_number: bool,
    pub require_job_name: bool,
    // --- credit controls (Manager / Admin only) ---
    pub charge_enabled: bool,
    pub credit_limit: Option<Decimal>,
    pub payment_terms: Option<String>,
    pub terms_days: Option<i32>,
    pub finance_rate_pct_monthly: Option<Decimal>,
    pub credit_hold: bool,
    pub portal_enabled: bool,
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Next free `C-NNNN` account number: one past the highest numeric
/// `C-` suffix in use (starting at `C-1001`).
fn next_account_number(existing: &[Customer]) -> String {
    let max = existing
        .iter()
        .filter_map(|c| c.account_number.strip_prefix("C-"))
        .filter_map(|n| n.parse::<u64>().ok())
        .max()
        .unwrap_or(1000);
    format!("C-{}", max.max(1000) + 1)
}

/// The credit-control fields of a customer, for comparing what a
/// request would change.
#[derive(PartialEq)]
struct CreditControls {
    charge_enabled: bool,
    credit_limit: Decimal,
    payment_terms: String,
    terms_days: Option<i32>,
    finance_rate_pct_monthly: Option<Decimal>,
    credit_hold: bool,
    portal_enabled: bool,
}

impl CreditControls {
    fn of(c: &Customer) -> Self {
        Self {
            charge_enabled: c.charge_enabled,
            credit_limit: c.credit_limit,
            payment_terms: c.payment_terms.clone(),
            terms_days: c.terms_days,
            finance_rate_pct_monthly: c.finance_rate_pct_monthly,
            credit_hold: c.credit_hold,
            portal_enabled: c.portal_enabled,
        }
    }
}

impl Edge {
    /// Validate `input` and apply it onto `customer` (a fresh
    /// record on create, the stored one on update).
    async fn apply_customer_input(
        &self,
        customer: &mut Customer,
        input: CustomerInput,
    ) -> Result<(), EdgeError> {
        let customer_type = clean(input.customer_type)
            .map(|t| t.to_lowercase())
            .unwrap_or_else(|| customer.customer_type.clone());
        let customer_type = if customer_type.is_empty() {
            "retail".to_string()
        } else {
            customer_type
        };
        if !CUSTOMER_TYPES.contains(&customer_type.as_str()) {
            return Err(EdgeError::InvalidInput(format!(
                "customer type must be one of {}",
                CUSTOMER_TYPES.join(", ")
            )));
        }
        let company_name = clean(input.company_name);
        let first_name = clean(input.first_name);
        let last_name = clean(input.last_name);
        if company_name.is_none() && first_name.is_none() && last_name.is_none() {
            return Err(EdgeError::InvalidInput(
                "a company name or a first/last name is required".to_string(),
            ));
        }
        let email = clean(input.email);
        if let Some(email) = &email {
            let valid = email
                .split_once('@')
                .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
            if !valid {
                return Err(EdgeError::InvalidInput(format!(
                    "{email} is not a valid email address"
                )));
            }
        }
        if let Some(level_id) = input.price_level_id {
            df_db::get_price_level(self.pool(), level_id)
                .await
                .map_err(|_| EdgeError::InvalidInput(format!("unknown price level {level_id}")))?;
        }
        let credit_limit = input.credit_limit.unwrap_or(Decimal::ZERO);
        if credit_limit < Decimal::ZERO {
            return Err(EdgeError::InvalidInput(
                "credit limit cannot be negative".to_string(),
            ));
        }
        if input.terms_days.is_some_and(|d| !(0..=365).contains(&d)) {
            return Err(EdgeError::InvalidInput(
                "terms days must be between 0 and 365".to_string(),
            ));
        }
        if input
            .finance_rate_pct_monthly
            .is_some_and(|r| r < Decimal::ZERO || r > Decimal::from(100))
        {
            return Err(EdgeError::InvalidInput(
                "finance rate must be between 0 and 100 percent per month".to_string(),
            ));
        }

        if let Some(account_number) = clean(input.account_number) {
            if account_number != customer.account_number {
                match df_db::get_customer_by_account(
                    self.pool(),
                    self.config().org_id,
                    &account_number,
                )
                .await
                {
                    Ok(other) if other.id != customer.id => {
                        return Err(EdgeError::InvalidInput(format!(
                            "account number {account_number} is already in use"
                        )));
                    }
                    Ok(_) | Err(df_db::DbError::NotFound(_)) => {}
                    Err(err) => return Err(err.into()),
                }
                customer.account_number = account_number;
            }
        }

        customer.customer_type = customer_type;
        customer.company_name = company_name;
        customer.first_name = first_name;
        customer.last_name = last_name;
        customer.email = email;
        customer.phone = clean(input.phone);
        customer.address_line1 = clean(input.address_line1);
        customer.address_line2 = clean(input.address_line2);
        customer.city = clean(input.city);
        customer.province = clean(input.province);
        customer.postal_code = clean(input.postal_code).map(|p| p.to_uppercase());
        customer.notes = clean(input.notes);
        customer.price_level_id = input.price_level_id;
        customer.tax_exempt_cert = clean(input.tax_exempt_cert);
        customer.tax_exempt_expires = input.tax_exempt_expires;
        customer.require_po_number = input.require_po_number;
        customer.require_job_name = input.require_job_name;
        customer.charge_enabled = input.charge_enabled;
        customer.credit_limit = credit_limit;
        customer.payment_terms = clean(input.payment_terms)
            .map(|t| t.to_uppercase())
            .unwrap_or_else(|| DEFAULT_PAYMENT_TERMS.to_string());
        customer.terms_days = input.terms_days;
        customer.finance_rate_pct_monthly = input.finance_rate_pct_monthly;
        customer.credit_hold = input.credit_hold;
        customer.portal_enabled = input.portal_enabled;
        Ok(())
    }

    /// Create a customer (`POST /pos/customers`). `allow_credit`
    /// must be `true` to set any credit control away from its
    /// default (no charge account, zero limit, `NET30`, no hold, no
    /// portal).
    pub async fn create_customer(
        &self,
        input: CustomerInput,
        allow_credit: bool,
    ) -> Result<Customer, EdgeError> {
        let mut customer = Customer {
            id: Uuid::now_v7(),
            org_id: self.config().org_id,
            account_number: String::new(),
            customer_type: "retail".to_string(),
            company_name: None,
            first_name: None,
            last_name: None,
            email: None,
            phone: None,
            price_level_id: None,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: false,
            credit_limit: Decimal::ZERO,
            payment_terms: DEFAULT_PAYMENT_TERMS.to_string(),
            terms_days: None,
            finance_rate_pct_monthly: None,
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: false,
            address_line1: None,
            address_line2: None,
            city: None,
            province: None,
            postal_code: None,
            notes: None,
        };
        let defaults = CreditControls::of(&customer);
        self.apply_customer_input(&mut customer, input).await?;
        if !allow_credit && CreditControls::of(&customer) != defaults {
            return Err(EdgeError::Forbidden(
                "only a manager or admin can set charge-account / credit settings".to_string(),
            ));
        }
        if customer.account_number.is_empty() {
            let existing = df_db::list_customers_for_org(self.pool(), self.config().org_id).await?;
            customer.account_number = next_account_number(&existing);
        }
        df_db::insert_customer(self.pool(), &customer).await?;
        Ok(df_db::get_customer(self.pool(), customer.id).await?)
    }

    /// Replace a customer's editable fields (`POST
    /// /pos/customers/{id}`). An unknown id, or one from another
    /// org, is a 404. `allow_credit` must be `true` to change any
    /// credit control.
    pub async fn update_customer(
        &self,
        id: Uuid,
        input: CustomerInput,
        allow_credit: bool,
    ) -> Result<Customer, EdgeError> {
        let mut customer = df_db::get_customer(self.pool(), id).await?;
        if customer.org_id != self.config().org_id {
            return Err(EdgeError::NotFound(format!("customer {id}")));
        }
        let before = CreditControls::of(&customer);
        self.apply_customer_input(&mut customer, input).await?;
        if !allow_credit && CreditControls::of(&customer) != before {
            return Err(EdgeError::Forbidden(
                "only a manager or admin can change charge-account / credit settings".to_string(),
            ));
        }
        df_db::update_customer(self.pool(), &customer).await?;
        Ok(df_db::get_customer(self.pool(), id).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_account(n: &str) -> Customer {
        Customer {
            id: Uuid::now_v7(),
            org_id: Uuid::nil(),
            account_number: n.to_string(),
            customer_type: "retail".to_string(),
            company_name: None,
            first_name: None,
            last_name: None,
            email: None,
            phone: None,
            price_level_id: None,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: false,
            credit_limit: Decimal::ZERO,
            payment_terms: DEFAULT_PAYMENT_TERMS.to_string(),
            terms_days: None,
            finance_rate_pct_monthly: None,
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: false,
            address_line1: None,
            address_line2: None,
            city: None,
            province: None,
            postal_code: None,
            notes: None,
        }
    }

    #[test]
    fn next_account_number_starts_at_1001_and_skips_non_numeric() {
        assert_eq!(next_account_number(&[]), "C-1001");
        let existing = vec![
            with_account("C-1001"),
            with_account("C-1042"),
            with_account("C-A"),
            with_account("WALKIN"),
        ];
        assert_eq!(next_account_number(&existing), "C-1043");
    }
}
