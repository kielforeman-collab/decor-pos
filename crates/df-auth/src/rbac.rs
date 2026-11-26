//! Role-based access control (spec §1: RBAC, manager overrides).
//!
//! Roles are deliberately coarse in Phase 0 — the counter needs fast,
//! auditable answers to "can this user void / override a price / run a
//! statement". Finer-grained permissions are a later-phase extension.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Well-known `roles` ids (migration `0012_phase7` seeds a `roles`
/// table with exactly these ids and names). `users.role_id` is a
/// UUID, not a `roles` table FK in the spec §5.1 sense — there was
/// never a `roles` table in Phases 0–6, and no code ever wrote a
/// `role_id` — so Phase 7 defines the mapping here, seeds the table
/// with it, and resolves it in Rust. The ids are fixed literals (not
/// derived from the role name) so a rename of a variant never
/// silently re-points existing users. No FK constraint is added on
/// `users.role_id`: a deployment that hand-inserted a user with an
/// arbitrary `role_id` must keep migrating, and the app rejects an
/// unmapped id at read time instead.
pub fn role_id(role: Role) -> Uuid {
    // Fixed v4-format literals, one per role, in declaration order.
    let text = match role {
        Role::Cashier => "10000000-0000-4000-8000-000000000001",
        Role::SalesAssociate => "10000000-0000-4000-8000-000000000002",
        Role::Warehouse => "10000000-0000-4000-8000-000000000003",
        Role::Driver => "10000000-0000-4000-8000-000000000004",
        Role::Manager => "10000000-0000-4000-8000-000000000005",
        Role::Admin => "10000000-0000-4000-8000-000000000006",
        Role::PortalContractor => "10000000-0000-4000-8000-000000000007",
    };
    Uuid::parse_str(text).expect("role id literal is a valid UUID")
}

/// Inverse of [`role_id`]: `None` for an id no role owns (e.g. a
/// hand-inserted user from before Phase 7). Callers treat `None` as
/// invalid data, never as a default role.
pub fn role_from_id(id: Uuid) -> Option<Role> {
    [
        Role::Cashier,
        Role::SalesAssociate,
        Role::Warehouse,
        Role::Driver,
        Role::Manager,
        Role::Admin,
        Role::PortalContractor,
    ]
    .into_iter()
    .find(|role| role_id(*role) == id)
}

/// Permissions checked at the counter and in back-office.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    PosCheckout,
    PosVoid,
    PosPriceOverride,
    PosTillManage,
    InventoryView,
    InventoryAdjust,
    PoCreate,
    PoReceive,
    ArView,
    ArPayment,
    ArStatementRun,
    CustomerManage,
    PricingManage,
    UserManage,
    ReportView,
    SystemAdmin,
    /// View schedules, forecasts, coverage and timeclock entries.
    SchedulingView,
    /// Manage shift templates, shifts and statuses.
    SchedulingManage,
    /// Push edge outbox batches to HQ (`POST /api/v1/sync/ingest`).
    /// Held by Manager and Admin only. The ingest endpoint takes a
    /// staff session `Bearer` token, not a separate store service
    /// token (Phase 8 decision — see the endpoint's doc comment):
    /// sessions already exist, expire in 8h and are revocable, so
    /// no new credential type, storage, issuance or rotation is
    /// needed; the sink is idempotent, which bounds the blast radius.
    SyncIngest,
}

/// Roles. A `Manager` can do everything a cashier can, plus overrides;
/// `Admin` can do everything except nothing — it holds `SystemAdmin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    Cashier,
    SalesAssociate,
    Warehouse,
    Driver,
    Manager,
    Admin,
    /// A contractor using the 24/7 portal (scoped, Phase 5).
    PortalContractor,
}

impl Role {
    /// The canonical name stored in `sessions.role` (the cached role
    /// text) and accepted by `df-server users create --role`.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Cashier => "Cashier",
            Role::SalesAssociate => "SalesAssociate",
            Role::Warehouse => "Warehouse",
            Role::Driver => "Driver",
            Role::Manager => "Manager",
            Role::Admin => "Admin",
            Role::PortalContractor => "PortalContractor",
        }
    }

    pub fn permissions(self) -> &'static [Permission] {
        use Permission::*;
        match self {
            Role::Cashier => &[PosCheckout, PosTillManage, CustomerManage, SchedulingView],
            Role::SalesAssociate => &[
                PosCheckout,
                PosTillManage,
                CustomerManage,
                InventoryView,
                ArView,
                SchedulingView,
            ],
            Role::Warehouse => &[InventoryView, InventoryAdjust, PoReceive, SchedulingView],
            Role::Driver => &[InventoryView, SchedulingView],
            Role::Manager => &[
                PosCheckout,
                PosVoid,
                PosPriceOverride,
                PosTillManage,
                InventoryView,
                InventoryAdjust,
                PoCreate,
                PoReceive,
                ArView,
                ArPayment,
                ArStatementRun,
                CustomerManage,
                PricingManage,
                ReportView,
                SchedulingView,
                SchedulingManage,
                SyncIngest,
            ],
            Role::Admin => &[
                PosCheckout,
                PosVoid,
                PosPriceOverride,
                PosTillManage,
                InventoryView,
                InventoryAdjust,
                PoCreate,
                PoReceive,
                ArView,
                ArPayment,
                ArStatementRun,
                CustomerManage,
                PricingManage,
                UserManage,
                ReportView,
                SystemAdmin,
                SchedulingView,
                SchedulingManage,
                SyncIngest,
            ],
            // Portal contractors hold NO staff permissions: their
            // only access is the Portal rule (role == PortalContractor
            // + linked customer_id). They previously held ArView,
            // which let a contractor read any customer's AR summary
            // through the unscoped staff AR routes — fixed in Phase 7.
            Role::PortalContractor => &[],
        }
    }

    pub fn has_permission(self, permission: Permission) -> bool {
        self.permissions().contains(&permission)
    }

    /// Can this role approve a manager override (price below margin
    /// floor, credit hold, void)? Only Manager and Admin.
    pub fn can_approve_override(self) -> bool {
        matches!(self, Role::Manager | Role::Admin)
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "Cashier" => Ok(Role::Cashier),
            "SalesAssociate" => Ok(Role::SalesAssociate),
            "Warehouse" => Ok(Role::Warehouse),
            "Driver" => Ok(Role::Driver),
            "Manager" => Ok(Role::Manager),
            "Admin" => Ok(Role::Admin),
            "PortalContractor" => Ok(Role::PortalContractor),
            other => Err(format!(
                "unknown role {other:?} (Cashier | SalesAssociate | Warehouse | Driver | Manager | Admin | PortalContractor)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cashier_cannot_override_or_void() {
        assert!(!Role::Cashier.has_permission(Permission::PosPriceOverride));
        assert!(!Role::Cashier.has_permission(Permission::PosVoid));
        assert!(Role::Cashier.has_permission(Permission::PosCheckout));
        assert!(!Role::Cashier.can_approve_override());
    }

    #[test]
    fn manager_can_override_but_not_administer_system() {
        assert!(Role::Manager.has_permission(Permission::PosPriceOverride));
        assert!(Role::Manager.has_permission(Permission::ArStatementRun));
        assert!(Role::Manager.can_approve_override());
        assert!(!Role::Manager.has_permission(Permission::SystemAdmin));
        assert!(!Role::Manager.has_permission(Permission::UserManage));
    }

    #[test]
    fn admin_has_everything() {
        for permission in [
            Permission::PosCheckout,
            Permission::PosVoid,
            Permission::PosPriceOverride,
            Permission::PosTillManage,
            Permission::InventoryView,
            Permission::InventoryAdjust,
            Permission::PoCreate,
            Permission::PoReceive,
            Permission::ArView,
            Permission::ArPayment,
            Permission::ArStatementRun,
            Permission::CustomerManage,
            Permission::PricingManage,
            Permission::UserManage,
            Permission::ReportView,
            Permission::SystemAdmin,
            Permission::SchedulingView,
            Permission::SchedulingManage,
            Permission::SyncIngest,
        ] {
            assert!(
                Role::Admin.has_permission(permission),
                "admin missing {permission:?}"
            );
        }
    }

    #[test]
    fn portal_contractor_holds_no_staff_permissions() {
        // Phase 7: a contractor's only access is the Portal rule
        // (role + linked customer). ArView used to leak any
        // customer's AR summary through the staff AR routes.
        for permission in [
            Permission::PosCheckout,
            Permission::ArView,
            Permission::ArPayment,
            Permission::ReportView,
            Permission::SchedulingView,
            Permission::SchedulingManage,
            Permission::UserManage,
        ] {
            assert!(
                !Role::PortalContractor.has_permission(permission),
                "portal contractor must not hold {permission:?}"
            );
        }
    }

    #[test]
    fn scheduling_permissions() {
        // Every staff role can view schedules and clock in/out;
        // only Manager/Admin manage templates and shifts.
        for role in [
            Role::Cashier,
            Role::SalesAssociate,
            Role::Warehouse,
            Role::Driver,
            Role::Manager,
            Role::Admin,
        ] {
            assert!(role.has_permission(Permission::SchedulingView));
        }
        assert!(!Role::Cashier.has_permission(Permission::SchedulingManage));
        assert!(!Role::Warehouse.has_permission(Permission::SchedulingManage));
        assert!(Role::Manager.has_permission(Permission::SchedulingManage));
        assert!(Role::Admin.has_permission(Permission::SchedulingManage));
    }

    #[test]
    fn warehouse_and_driver_are_scoped() {
        assert!(Role::Warehouse.has_permission(Permission::PoReceive));
        assert!(!Role::Warehouse.has_permission(Permission::PosCheckout));
        assert!(!Role::Driver.has_permission(Permission::ArPayment));
    }

    #[test]
    fn sync_ingest_permission() {
        // Only Manager and Admin may push outbox batches to HQ
        // (Phase 8 ingest endpoint). Cashiers run the till but do
        // not sync; portal contractors hold no staff permissions.
        assert!(Role::Manager.has_permission(Permission::SyncIngest));
        assert!(Role::Admin.has_permission(Permission::SyncIngest));
        assert!(!Role::Cashier.has_permission(Permission::SyncIngest));
        assert!(!Role::SalesAssociate.has_permission(Permission::SyncIngest));
        assert!(!Role::Warehouse.has_permission(Permission::SyncIngest));
        assert!(!Role::Driver.has_permission(Permission::SyncIngest));
        assert!(!Role::PortalContractor.has_permission(Permission::SyncIngest));
    }

    #[test]
    fn role_ids_round_trip_and_are_distinct() {
        let roles = [
            Role::Cashier,
            Role::SalesAssociate,
            Role::Warehouse,
            Role::Driver,
            Role::Manager,
            Role::Admin,
            Role::PortalContractor,
        ];
        let mut ids = Vec::new();
        for role in roles {
            assert_eq!(role_from_id(role_id(role)), Some(role));
            assert_eq!(Role::from_str(role.as_str()).unwrap(), role);
            assert!(!ids.contains(&role_id(role)), "duplicate role id");
            ids.push(role_id(role));
        }
        // An id no role owns maps to nothing, never to a default.
        assert_eq!(
            role_from_id(Uuid::parse_str("10000000-0000-4000-8000-000000000099").unwrap()),
            None
        );
        assert!(Role::from_str("SuperUser").is_err());
    }
}
