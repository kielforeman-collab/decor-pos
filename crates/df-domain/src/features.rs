//! Feature flags per organization (spec §4 `enabled_modules`).
//!
//! `core_pos`, `paint_tinting`, `contractor_ar`, `inventory_po` and
//! `matrix_pricing` are on by default; the decorating verticals and
//! enterprise modules are opt-in flags.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Module flags from spec §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModuleFlag {
    CorePos,
    PaintTinting,
    ContractorAr,
    InventoryPo,
    MatrixPricing,
    VerticalFlooring,
    VerticalWindowTreatments,
    VerticalWallpaper,
    VerticalSampleLoans,
    CrmMarketing,
    DeliveryLogistics,
    ContractorPortalEcom,
    VendorEdi,
    SchedulingPlus,
}

impl ModuleFlag {
    pub fn key(self) -> &'static str {
        match self {
            ModuleFlag::CorePos => "core_pos",
            ModuleFlag::PaintTinting => "paint_tinting",
            ModuleFlag::ContractorAr => "contractor_ar",
            ModuleFlag::InventoryPo => "inventory_po",
            ModuleFlag::MatrixPricing => "matrix_pricing",
            ModuleFlag::VerticalFlooring => "vertical_flooring",
            ModuleFlag::VerticalWindowTreatments => "vertical_window_treatments",
            ModuleFlag::VerticalWallpaper => "vertical_wallpaper",
            ModuleFlag::VerticalSampleLoans => "vertical_sample_loans",
            ModuleFlag::CrmMarketing => "crm_marketing",
            ModuleFlag::DeliveryLogistics => "delivery_logistics",
            ModuleFlag::ContractorPortalEcom => "contractor_portal_ecom",
            ModuleFlag::VendorEdi => "vendor_edi",
            ModuleFlag::SchedulingPlus => "scheduling_plus",
        }
    }

    /// Flags that are on unless explicitly disabled.
    pub fn default_on(self) -> bool {
        matches!(
            self,
            ModuleFlag::CorePos
                | ModuleFlag::PaintTinting
                | ModuleFlag::ContractorAr
                | ModuleFlag::InventoryPo
                | ModuleFlag::MatrixPricing
        )
    }

    pub fn all() -> [ModuleFlag; 14] {
        [
            ModuleFlag::CorePos,
            ModuleFlag::PaintTinting,
            ModuleFlag::ContractorAr,
            ModuleFlag::InventoryPo,
            ModuleFlag::MatrixPricing,
            ModuleFlag::VerticalFlooring,
            ModuleFlag::VerticalWindowTreatments,
            ModuleFlag::VerticalWallpaper,
            ModuleFlag::VerticalSampleLoans,
            ModuleFlag::CrmMarketing,
            ModuleFlag::DeliveryLogistics,
            ModuleFlag::ContractorPortalEcom,
            ModuleFlag::VendorEdi,
            ModuleFlag::SchedulingPlus,
        ]
    }
}

/// The enabled-module set for one organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnabledModules(pub HashSet<ModuleFlag>);

impl Default for EnabledModules {
    fn default() -> Self {
        Self(
            ModuleFlag::all()
                .iter()
                .copied()
                .filter(|f| f.default_on())
                .collect(),
        )
    }
}

impl EnabledModules {
    pub fn is_enabled(&self, flag: ModuleFlag) -> bool {
        self.0.contains(&flag)
    }

    pub fn enable(&mut self, flag: ModuleFlag) {
        self.0.insert(flag);
    }

    pub fn disable(&mut self, flag: ModuleFlag) {
        self.0.remove(&flag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let modules = EnabledModules::default();
        assert!(modules.is_enabled(ModuleFlag::CorePos));
        assert!(modules.is_enabled(ModuleFlag::PaintTinting));
        assert!(!modules.is_enabled(ModuleFlag::VerticalFlooring));
        assert!(!modules.is_enabled(ModuleFlag::VendorEdi));
    }
}
