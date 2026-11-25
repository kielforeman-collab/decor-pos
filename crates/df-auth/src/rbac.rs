//! Role-based access control (spec §1: RBAC, manager overrides).
//!
//! Roles are deliberately coarse in Phase 0 — the counter needs fast,
//! auditable answers to "can this user void / override a price / run a
//! statement". Finer-grained permissions are a later-phase extension.

use serde::{Deserialize, Serialize};

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
    pub fn permissions(self) -> &'static [Permission] {
        use Permission::*;
        match self {
            Role::Cashier => &[PosCheckout, PosTillManage, CustomerManage],
            Role::SalesAssociate => &[
                PosCheckout,
                PosTillManage,
                CustomerManage,
                InventoryView,
                ArView,
            ],
            Role::Warehouse => &[InventoryView, InventoryAdjust, PoReceive],
            Role::Driver => &[InventoryView],
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
            ],
            Role::PortalContractor => &[ArView],
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
        ] {
            assert!(
                Role::Admin.has_permission(permission),
                "admin missing {permission:?}"
            );
        }
    }

    #[test]
    fn warehouse_and_driver_are_scoped() {
        assert!(Role::Warehouse.has_permission(Permission::PoReceive));
        assert!(!Role::Warehouse.has_permission(Permission::PosCheckout));
        assert!(!Role::Driver.has_permission(Permission::ArPayment));
    }
}
