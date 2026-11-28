//! Edge Admin HTTP handlers: store settings & tax/eco-fee jurisdiction,
//! staff/user management, and store service tokens (`dfs_...`).
//!
//! RBAC gates (wired in `http/mod.rs`):
//! * `GET /admin/store` — `Permission::SyncIngest` (Manager / Admin)
//! * `POST /admin/store` — `Permission::SystemAdmin` (Admin only)
//! * `GET /admin/users`, `POST /admin/users`, `POST /admin/users/{id}`,
//!   `POST /admin/users/{id}/revoke-sessions` — `Permission::UserManage` (Admin only)
//! * `GET /admin/service-tokens`, `POST /admin/service-tokens`,
//!   `POST /admin/service-tokens/{id}/revoke` — `Permission::UserManage` (Admin only)
//!
//! Security invariants:
//! * Password, PIN, badge, and service-token hashes are NEVER returned.
//! * Raw `dfs_` service tokens are returned exactly once on `201 Created`.
//! * An admin cannot deactivate or demote their own active account.

use std::str::FromStr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use chrono::Duration;
use df_auth::{
    Role, SCOPE_SYNC_INGEST, ServiceToken, hash_badge_token, hash_password, hash_pin, role_from_id,
};
use df_auth_http::AuthSession;
use df_db::models::{Store, UserRow};
use df_db::{DbError, NewUser};
use df_domain::tax::{Country, eco_fee_presets, find_tax_rule, tax_presets};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ErrorResponse, HandlerResult, err_response};
use crate::edge::Edge;
use crate::error::EdgeError;

// --- Store settings & tax/eco jurisdiction --------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxPresetComponentView {
    pub kind: String,
    /// Exact decimal rate fraction (e.g. `"0.0725"` for 7.25%).
    pub rate: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxJurisdictionPresetView {
    pub country_code: String,
    pub region_code: String,
    pub total_rate: String,
    pub components: Vec<TaxPresetComponentView>,
    pub eco_fee_rule_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreSettingsResponse {
    pub store: Store,
    /// Active preset rule matched by `(store.country_code, store.region_code)`,
    /// or `None` when the jurisdiction has no built-in tax preset (tax = 0).
    pub active_tax_preset: Option<TaxJurisdictionPresetView>,
    /// Number of built-in PaintCare / Canadian stewardship eco-fee rules
    /// matching `(store.country_code, store.region_code)`.
    pub active_eco_fee_rule_count: usize,
    /// All shipped US/CA tax/eco presets for UI guidance & selection.
    pub available_presets: Vec<TaxJurisdictionPresetView>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateStoreSettingsRequest {
    pub code: Option<String>,
    pub name: Option<String>,
    pub country_code: Option<String>,
    pub region_code: Option<String>,
    pub postal_code: Option<String>,
    pub currency: Option<String>,
    pub timezone: Option<String>,
    pub is_warehouse: Option<bool>,
}

fn country_code_str(country: Country) -> &'static str {
    match country {
        Country::Us => "US",
        Country::Ca => "CA",
    }
}

fn parse_country(code: &str) -> Option<Country> {
    match code {
        "US" => Some(Country::Us),
        "CA" => Some(Country::Ca),
        _ => None,
    }
}

fn build_store_settings_response(store: Store) -> StoreSettingsResponse {
    let presets = tax_presets();
    let eco_rules = eco_fee_presets();

    let count_eco = |country: Country, region: &str| -> usize {
        eco_rules
            .iter()
            .filter(|r| r.country == country && r.region == region)
            .count()
    };

    let available_presets: Vec<TaxJurisdictionPresetView> = presets
        .iter()
        .map(|rule| TaxJurisdictionPresetView {
            country_code: country_code_str(rule.country).to_string(),
            region_code: rule.region.clone(),
            total_rate: rule.total_rate().to_string(),
            components: rule
                .components
                .iter()
                .map(|c| TaxPresetComponentView {
                    kind: c.kind.code().to_string(),
                    rate: c.rate.to_string(),
                })
                .collect(),
            eco_fee_rule_count: count_eco(rule.country, &rule.region),
        })
        .collect();

    let (active_tax_preset, active_eco_fee_rule_count) = match parse_country(&store.country_code) {
        Some(country) => {
            let eco_count = count_eco(country, &store.region_code);
            let tax = find_tax_rule(&presets, country, &store.region_code).map(|rule| {
                TaxJurisdictionPresetView {
                    country_code: country_code_str(rule.country).to_string(),
                    region_code: rule.region.clone(),
                    total_rate: rule.total_rate().to_string(),
                    components: rule
                        .components
                        .iter()
                        .map(|c| TaxPresetComponentView {
                            kind: c.kind.code().to_string(),
                            rate: c.rate.to_string(),
                        })
                        .collect(),
                    eco_fee_rule_count: eco_count,
                }
            });
            (tax, eco_count)
        }
        None => (None, 0),
    };

    StoreSettingsResponse {
        store,
        active_tax_preset,
        active_eco_fee_rule_count,
        available_presets,
    }
}

pub(super) async fn get_store_settings(
    State(edge): State<Arc<Edge>>,
) -> HandlerResult<StoreSettingsResponse> {
    let store = match df_db::get_store(edge.pool(), edge.config().store_id).await {
        Ok(store) => store,
        Err(DbError::NotFound(_)) => {
            edge.ensure_configured_store(None, None)
                .await
                .map_err(err_response)?;
            df_db::get_store(edge.pool(), edge.config().store_id)
                .await
                .map_err(|e| err_response(e.into()))?
        }
        Err(err) => return Err(err_response(err.into())),
    };
    Ok(Json(build_store_settings_response(store)))
}

pub(super) async fn update_store_settings(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<UpdateStoreSettingsRequest>,
) -> HandlerResult<StoreSettingsResponse> {
    let mut store = match df_db::get_store(edge.pool(), edge.config().store_id).await {
        Ok(store) => store,
        Err(DbError::NotFound(_)) => {
            edge.ensure_configured_store(None, None)
                .await
                .map_err(err_response)?;
            df_db::get_store(edge.pool(), edge.config().store_id)
                .await
                .map_err(|e| err_response(e.into()))?
        }
        Err(err) => return Err(err_response(err.into())),
    };

    if let Some(code) = body.code {
        let trimmed = code.trim().to_ascii_uppercase();
        if trimmed.is_empty() {
            return Err(err_response(EdgeError::InvalidInput(
                "store code must not be blank".to_string(),
            )));
        }
        store.code = trimmed;
    }
    if let Some(name) = body.name {
        let trimmed = name.trim().to_string();
        if trimmed.is_empty() {
            return Err(err_response(EdgeError::InvalidInput(
                "store name must not be blank".to_string(),
            )));
        }
        store.name = trimmed;
    }
    if let Some(country_code) = body.country_code {
        store.country_code = country_code.trim().to_ascii_uppercase();
    }
    if let Some(region_code) = body.region_code {
        store.region_code = region_code.trim().to_ascii_uppercase();
    }
    if let Some(postal_code) = body.postal_code {
        store.postal_code = postal_code.trim().to_string();
    }
    if let Some(currency) = body.currency {
        let normalized = currency.trim().to_ascii_uppercase();
        if normalized != "USD" && normalized != "CAD" {
            return Err(err_response(EdgeError::InvalidInput(format!(
                "unsupported currency {normalized:?} (expected USD or CAD)"
            ))));
        }
        store.currency = normalized;
    }
    if let Some(timezone) = body.timezone {
        let trimmed = timezone.trim().to_string();
        if trimmed.is_empty() {
            return Err(err_response(EdgeError::InvalidInput(
                "timezone must not be blank".to_string(),
            )));
        }
        store.timezone = trimmed;
    }
    if let Some(is_warehouse) = body.is_warehouse {
        store.is_warehouse = is_warehouse;
    }

    df_db::update_store(edge.pool(), &store)
        .await
        .map_err(|e| err_response(e.into()))?;

    Ok(Json(build_store_settings_response(store)))
}

// --- Staff & user administration (`Permission::UserManage`) ---------------

/// Safe projection of a `users` row for admin screens — NEVER includes
/// password, PIN, or badge hashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUserSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub active: bool,
    pub has_password: bool,
    pub has_pin: bool,
    pub has_badge: bool,
    pub portal_customer_id: Option<Uuid>,
}

impl AdminUserSummary {
    fn from_row(row: UserRow) -> Self {
        let role = role_from_id(row.role_id)
            .map(Role::as_str)
            .unwrap_or("Unknown")
            .to_string();
        Self {
            id: row.id,
            org_id: row.org_id,
            email: row.email,
            display_name: row.display_name,
            role,
            active: row.active,
            has_password: row.password_hash.is_some(),
            has_pin: row.counter_pin_hash.is_some(),
            has_badge: row.badge_token_hash.is_some(),
            portal_customer_id: row.portal_customer_id,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateAdminUserRequest {
    pub email: String,
    pub display_name: Option<String>,
    pub role: String,
    pub password: Option<String>,
    pub pin: Option<String>,
    pub badge_token: Option<String>,
    pub portal_customer_id: Option<Uuid>,
    pub active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAdminUserRequest {
    pub display_name: Option<String>,
    pub role: Option<String>,
    pub active: Option<bool>,
    pub password: Option<String>,
    pub pin: Option<String>,
    #[serde(default)]
    pub clear_pin: bool,
    pub badge_token: Option<String>,
    #[serde(default)]
    pub clear_badge: bool,
    pub portal_customer_id: Option<Option<Uuid>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RevokeUserSessionsResponse {
    pub user_id: Uuid,
    pub revoked_count: u64,
}

fn parse_role(text: &str) -> Result<Role, (StatusCode, Json<ErrorResponse>)> {
    Role::from_str(text.trim()).map_err(|_| {
        err_response(EdgeError::InvalidInput(format!(
            "unknown role {:?} (expected Cashier, SalesAssociate, Warehouse, Driver, Manager, Admin, or PortalContractor)",
            text
        )))
    })
}

fn is_unique_email_err(err: &DbError) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("unique") || msg.contains("duplicate key")
}

pub(super) async fn list_admin_users(
    State(edge): State<Arc<Edge>>,
) -> HandlerResult<Vec<AdminUserSummary>> {
    let rows = df_db::list_users_for_org(edge.pool(), edge.config().org_id)
        .await
        .map_err(|e| err_response(e.into()))?;
    Ok(Json(
        rows.into_iter().map(AdminUserSummary::from_row).collect(),
    ))
}

pub(super) async fn create_admin_user(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateAdminUserRequest>,
) -> Result<(StatusCode, Json<AdminUserSummary>), (StatusCode, Json<ErrorResponse>)> {
    let email = body.email.trim().to_string();
    if email.is_empty() {
        return Err(err_response(EdgeError::InvalidInput(
            "email must not be blank".to_string(),
        )));
    }
    let role = parse_role(&body.role)?;
    if role == Role::PortalContractor && body.portal_customer_id.is_none() {
        return Err(err_response(EdgeError::InvalidInput(
            "PortalContractor users require a portal_customer_id".to_string(),
        )));
    }
    if role != Role::PortalContractor && body.portal_customer_id.is_some() {
        return Err(err_response(EdgeError::InvalidInput(
            "portal_customer_id is only allowed for PortalContractor users".to_string(),
        )));
    }

    let password_hash = match body.password.as_deref().filter(|s| !s.is_empty()) {
        Some(pw) => Some(hash_password(pw).map_err(|e| {
            err_response(EdgeError::InvalidInput(format!("invalid password: {e}")))
        })?),
        None => None,
    };
    let counter_pin_hash = match body.pin.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(pin) => Some(
            hash_pin(pin.trim())
                .map_err(|e| err_response(EdgeError::InvalidInput(format!("invalid PIN: {e}"))))?,
        ),
        None => None,
    };
    let badge_token_hash = match body.badge_token.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(badge) => Some(hash_badge_token(badge.trim()).map_err(|e| {
            err_response(EdgeError::InvalidInput(format!("invalid badge token: {e}")))
        })?),
        None => None,
    };
    if password_hash.is_none() && counter_pin_hash.is_none() && badge_token_hash.is_none() {
        return Err(err_response(EdgeError::InvalidInput(
            "provide at least a password, counter PIN, or badge token".to_string(),
        )));
    }

    let display_name = body
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| email.split('@').next().unwrap_or(&email).to_string());

    let new_user = NewUser {
        org_id: edge.config().org_id,
        email,
        display_name,
        password_hash,
        counter_pin_hash,
        badge_token_hash,
        role,
        portal_customer_id: body.portal_customer_id,
        active: body.active.unwrap_or(true),
    };

    let created = df_db::create_user(edge.pool(), &new_user)
        .await
        .map_err(|e| {
            if is_unique_email_err(&e) {
                (
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: format!("a user with email {:?} already exists", new_user.email),
                    }),
                )
            } else {
                err_response(e.into())
            }
        })?;

    Ok((
        StatusCode::CREATED,
        Json(AdminUserSummary::from_row(created)),
    ))
}

pub(super) async fn update_admin_user(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAdminUserRequest>,
) -> HandlerResult<AdminUserSummary> {
    let existing = df_db::get_user_by_id(edge.pool(), id)
        .await
        .map_err(|e| err_response(e.into()))?;
    if existing.org_id != edge.config().org_id {
        return Err(err_response(EdgeError::NotFound(format!("user {id}"))));
    }
    let current_role = role_from_id(existing.role_id).ok_or_else(|| {
        err_response(EdgeError::InvalidInput(format!(
            "user {id} has an unrecognized role_id"
        )))
    })?;

    let target_role = match body.role.as_deref() {
        Some(role_text) => parse_role(role_text)?,
        None => current_role,
    };

    // Guardrails: an admin cannot lock themselves out.
    if auth.user_id == id {
        if body.active == Some(false) {
            return Err(err_response(EdgeError::InvalidInput(
                "cannot deactivate your own active admin account".to_string(),
            )));
        }
        if target_role != Role::Admin {
            return Err(err_response(EdgeError::InvalidInput(
                "cannot remove the Admin role from your own active account".to_string(),
            )));
        }
    }

    let target_display_name = match body.display_name {
        Some(name) => {
            let trimmed = name.trim().to_string();
            if trimmed.is_empty() {
                return Err(err_response(EdgeError::InvalidInput(
                    "display_name must not be blank".to_string(),
                )));
            }
            trimmed
        }
        None => existing.display_name.clone(),
    };

    let target_portal_customer_id = match body.portal_customer_id {
        Some(opt) => opt,
        None => existing.portal_customer_id,
    };
    if target_role == Role::PortalContractor && target_portal_customer_id.is_none() {
        return Err(err_response(EdgeError::InvalidInput(
            "PortalContractor users require a portal_customer_id".to_string(),
        )));
    }
    if target_role != Role::PortalContractor && target_portal_customer_id.is_some() {
        return Err(err_response(EdgeError::InvalidInput(
            "portal_customer_id is only allowed for PortalContractor users".to_string(),
        )));
    }

    // Validate new secrets up front before mutating anything.
    let new_password_hash = match body.password.as_deref().filter(|s| !s.is_empty()) {
        Some(pw) => Some(hash_password(pw).map_err(|e| {
            err_response(EdgeError::InvalidInput(format!("invalid password: {e}")))
        })?),
        None => None,
    };
    let new_pin_hash = match body.pin.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(pin) => Some(
            hash_pin(pin.trim())
                .map_err(|e| err_response(EdgeError::InvalidInput(format!("invalid PIN: {e}"))))?,
        ),
        None => None,
    };
    let new_badge_hash = match body.badge_token.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(badge) => Some(hash_badge_token(badge.trim()).map_err(|e| {
            err_response(EdgeError::InvalidInput(format!("invalid badge token: {e}")))
        })?),
        None => None,
    };

    if target_display_name != existing.display_name || target_role != current_role {
        df_db::update_user_profile(edge.pool(), id, &target_display_name, target_role)
            .await
            .map_err(|e| err_response(e.into()))?;
        // Revoke active sessions if the role changed so cached session
        // permissions cannot outlive a role downgrade.
        if target_role != current_role {
            let _ = df_db::revoke_sessions_for_user(edge.pool(), id).await;
        }
    }
    if target_portal_customer_id != existing.portal_customer_id {
        df_db::set_portal_customer(edge.pool(), id, target_portal_customer_id)
            .await
            .map_err(|e| err_response(e.into()))?;
    }
    if let Some(active) = body.active
        && active != existing.active
    {
        df_db::set_user_active(edge.pool(), id, active)
            .await
            .map_err(|e| err_response(e.into()))?;
        if !active {
            let _ = df_db::revoke_sessions_for_user(edge.pool(), id).await;
        }
    }
    if let Some(hash) = new_password_hash {
        df_db::set_password_hash(edge.pool(), id, Some(&hash))
            .await
            .map_err(|e| err_response(e.into()))?;
    }
    if body.clear_pin {
        df_db::set_pin_hash(edge.pool(), id, None)
            .await
            .map_err(|e| err_response(e.into()))?;
    } else if let Some(hash) = new_pin_hash {
        df_db::set_pin_hash(edge.pool(), id, Some(&hash))
            .await
            .map_err(|e| err_response(e.into()))?;
    }
    if body.clear_badge {
        df_db::set_badge_hash(edge.pool(), id, None)
            .await
            .map_err(|e| err_response(e.into()))?;
    } else if let Some(hash) = new_badge_hash {
        df_db::set_badge_hash(edge.pool(), id, Some(&hash))
            .await
            .map_err(|e| err_response(e.into()))?;
    }

    let updated = df_db::get_user_by_id(edge.pool(), id)
        .await
        .map_err(|e| err_response(e.into()))?;
    Ok(Json(AdminUserSummary::from_row(updated)))
}

pub(super) async fn revoke_admin_user_sessions(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<RevokeUserSessionsResponse> {
    let existing = df_db::get_user_by_id(edge.pool(), id)
        .await
        .map_err(|e| err_response(e.into()))?;
    if existing.org_id != edge.config().org_id {
        return Err(err_response(EdgeError::NotFound(format!("user {id}"))));
    }
    let revoked_count = df_db::revoke_sessions_for_user(edge.pool(), id)
        .await
        .map_err(|e| err_response(e.into()))?;
    Ok(Json(RevokeUserSessionsResponse {
        user_id: id,
        revoked_count,
    }))
}

// --- Store service tokens (`Permission::UserManage`) ----------------------

#[derive(Debug, Deserialize)]
pub struct CreateEdgeServiceTokenRequest {
    /// Target store UUID; defaults to the edge's own configured store.
    pub store_id: Option<Uuid>,
    pub label: String,
    /// Optional lifetime in days (`None` = non-expiring until revoked).
    pub ttl_days: Option<i64>,
}

/// Metadata summary for a store service token — NEVER includes `token_hash`
/// or raw secret material. Matches `df-api`'s `ServiceTokenSummary` wire shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeServiceTokenSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub label: String,
    pub scopes: Vec<String>,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
    pub last_used_at: Option<String>,
}

impl EdgeServiceTokenSummary {
    fn from_row(row: ServiceToken) -> Self {
        Self {
            id: row.id,
            org_id: row.org_id,
            store_id: row.store_id,
            label: row.label,
            scopes: row.scopes,
            created_at: row.created_at.to_rfc3339(),
            expires_at: row.expires_at.map(|d| d.to_rfc3339()),
            revoked_at: row.revoked_at.map(|d| d.to_rfc3339()),
            last_used_at: row.last_used_at.map(|d| d.to_rfc3339()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateEdgeServiceTokenResponse {
    pub token: String,
    #[serde(flatten)]
    pub metadata: EdgeServiceTokenSummary,
}

#[derive(Debug, Deserialize)]
pub struct EdgeServiceTokenQuery {
    pub store_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RevokeEdgeServiceTokenResponse {
    pub id: Uuid,
    pub revoked: bool,
}

pub(super) async fn list_admin_service_tokens(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<EdgeServiceTokenQuery>,
) -> HandlerResult<Vec<EdgeServiceTokenSummary>> {
    let rows = df_db::list_service_tokens(edge.pool(), edge.config().org_id, query.store_id)
        .await
        .map_err(|e| err_response(e.into()))?;
    Ok(Json(
        rows.into_iter()
            .map(EdgeServiceTokenSummary::from_row)
            .collect(),
    ))
}

pub(super) async fn create_admin_service_token(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateEdgeServiceTokenRequest>,
) -> Result<(StatusCode, Json<CreateEdgeServiceTokenResponse>), (StatusCode, Json<ErrorResponse>)> {
    let store_id = body.store_id.unwrap_or(edge.config().store_id);
    let ttl = match body.ttl_days {
        Some(days) if days <= 0 => {
            return Err(err_response(EdgeError::InvalidInput(
                "ttl_days must be a positive integer when provided".to_string(),
            )));
        }
        Some(days) => Some(Duration::days(days)),
        None => None,
    };
    let (raw, row) = df_db::create_service_token(
        edge.pool(),
        edge.config().org_id,
        store_id,
        &body.label,
        vec![SCOPE_SYNC_INGEST.to_string()],
        ttl,
    )
    .await
    .map_err(|e| match e {
        DbError::InvalidData(msg) => err_response(EdgeError::InvalidInput(msg)),
        other => err_response(other.into()),
    })?;
    Ok((
        StatusCode::CREATED,
        Json(CreateEdgeServiceTokenResponse {
            token: raw,
            metadata: EdgeServiceTokenSummary::from_row(row),
        }),
    ))
}

pub(super) async fn revoke_admin_service_token(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<RevokeEdgeServiceTokenResponse> {
    let revoked = df_db::revoke_service_token(edge.pool(), id)
        .await
        .map_err(|e| err_response(e.into()))?;
    if !revoked {
        return Err(err_response(EdgeError::NotFound(format!(
            "active service token {id} not found"
        ))));
    }
    Ok(Json(RevokeEdgeServiceTokenResponse { id, revoked }))
}
