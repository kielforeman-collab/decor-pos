//! Request, response, and query schemas for `df-api`.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

// --- schemas ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    /// Portal remember-me (30-day session) — only meaningful for a
    /// `PortalContractor`; staff sessions are always 8 hours.
    #[serde(default)]
    pub remember_me: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CounterSwitchRequest {
    /// Identifies the user: an Argon2id hash cannot be searched,
    /// so a bare PIN/badge cannot name its user (see
    /// `df_auth::login` docs).
    pub email: String,
    /// 4–6 digit counter PIN, or a badge token.
    pub pin: Option<String>,
    pub badge_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PasskeyVerifyRequest {
    pub user_id: String,
    pub response_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OverrideVerifyRequest {
    pub user_id: String,
    pub pin: String,
    pub reason_code: String,
}

#[derive(Debug, Deserialize)]
pub struct ColorSearchQuery {
    pub manufacturer: Option<String>,
    pub query: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FormulaQuery {
    pub query: Option<String>,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct StoreQuery {
    pub store_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ReportQuery {
    pub store_id: Option<Uuid>,
    pub date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ArAgingQuery {
    pub store_id: Option<Uuid>,
    pub as_of: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AppointmentListQuery {
    pub store_id: Option<Uuid>,
    pub date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DispatchQuery {
    pub store_id: Option<Uuid>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EdiDocumentsQuery {
    pub po_id: Uuid,
}

/// One customer's AR position (HQ read of the edge's summary shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArCustomerSummaryResponse {
    pub customer_id: String,
    pub account_number: String,
    pub company_name: Option<String>,
    pub customer_type: String,
    pub charge_enabled: bool,
    pub credit_limit: String,
    pub credit_hold: bool,
    pub balance: String,
    pub entry_count: usize,
    pub oldest_due_date: Option<NaiveDate>,
}

/// Per-customer aging row: buckets are
/// [current, 31-60, 61-90, 91-120, 120+], exact decimal strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgingCustomerResponse {
    pub customer_id: String,
    pub account_number: String,
    pub balance: String,
    pub buckets: [String; 5],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgingResponse {
    pub as_of: NaiveDate,
    pub totals: [String; 5],
    pub customers: Vec<AgingCustomerResponse>,
}
