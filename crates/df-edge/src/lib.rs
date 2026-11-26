//! `df-edge` — local store daemon (Phase 1, v0.2; spec §7).
//!
//! The edge daemon is the offline counter brain: it prices, checks
//! out, voids and returns sales against a local SQLite database,
//! opens/closes tills, parks holds, records quotes, prints receipts,
//! kicks the cash drawer, and drains the sync outbox to HQ when a
//! sink is available — with **no network use at all** while selling.
//!
//! Start here: [`EdgeConfig`] + [`Edge::open`]. The local HTTP/WS
//! API the POS talks to is [`Edge::router`] (routes documented in
//! [`http`]); [`Edge::serve`] runs it.
//!
//! Payments and hardware come from the real sibling crates:
//! [`payments`] re-exports `df_payments` (`PaymentProvider`,
//! `MockTerminal`, surcharge rules) and [`hw`] re-exports `df_hw`
//! (`Printer`, `MockPrinter`, ESC/POS `ReceiptBuilder`). Receipt
//! payments are mapped from `PaymentResult` to `df_hw`'s
//! `PaymentSummary` at the boundary in [`edge`].
//!
//! Money is `rust_decimal::Decimal` throughout — never floats. Card
//! surcharge default is 3% (see
//! [`EdgeConfig::default_surcharge_rules`]).
//!
//! Phase 4 (v0.5) adds the CRM, delivery and EDI wiring in
//! [`crm`], [`delivery`] and [`edi`]: activity log + tickler +
//! appointments + saved views + lapsed-contractor report, dispatch
//! board + packing slips + POD close-out, and X12 850 send /
//! 855-856-810-832 intake / 3-way match over the real `df-edi`
//! parsers and transports. The domain rules live in the sibling
//! crates `df-crm` (activities, reminders, appointments, saved
//! views, lapsed report) and `df-delivery` (dispatch state
//! machine, POD, packing slips); these modules are the wiring
//! layer (persistence + HTTP rules over the `df-db` repos), not a
//! second copy of that domain logic.
//!
//! Phase 6 (v1.0) adds the Scheduling+ wiring in [`scheduling`]:
//! shift templates, staff shifts, timeclock and the sales-driven
//! labor forecast + coverage check, over the `df-scheduling`
//! domain rules and the migration `0011_phase6` tables.

pub mod ar;
pub mod bi;
pub mod crm;
pub mod delivery;
pub mod edge;
pub mod edi;
pub mod error;
pub mod http;
pub mod hw;
pub mod inventory;
pub mod payments;
pub mod portal;
pub mod pricing;
pub mod scheduling;
pub mod tinting;
pub mod types;

pub use ar::PostedArPayment;
pub use crm::{
    CreateActivityRequest, CreateAppointmentRequest, CreateSavedViewRequest, LapsedReport,
    SavedViewCustomer, SnoozeActivityRequest, UpdateActivityRequest, UpdateAppointmentRequest,
};
pub use delivery::{
    AssignDeliveryRequest, CreateDeliveryRequest, DeliveryView, PackingSlipLine, PackingSlipView,
    SetDeliveryStatusRequest, SubmitPodRequest,
};
pub use df_ar::{CollectionsEntry, FinanceChargePosted, Statement};
pub use df_db::models::TillSession;
pub use df_edi::{
    InvoiceMatchReport, InvoiceMatchReportRow, PoAcknowledgmentRow, PriceCatalogApplicationRow,
    TransportRecord,
};
pub use edge::Edge;
pub use edi::{
    CatalogImportError, CatalogImportResponse, EDI_SENDER_ID, IntakeRequest, IntakeResponse,
    MarkPriceApplicationAppliedRequest, SendPoRequest, SendPoResponse,
};
pub use error::EdgeError;
pub use hw::{
    MockPrinter, PaymentSummary, Printer, ReceiptBuilder, ReceiptData, ReceiptLine,
    drawer_kick_bytes,
};
pub use inventory::{
    AgingCustomerRow, AgingView, ArCustomerSummary, BulkPricingRequest, CollectionsRow,
    CollectionsView, CountAdjustment, CountLineView, CountReconcileResponse, CreatePoLineRequest,
    CreatePurchaseOrderRequest, CreateTransferRequest, ImportProductsRequest, PhysicalCount,
    PoLineView, PricingRuleUpdate, PurchaseOrderView, ReceivePoLineRequest,
    ReceivePurchaseOrderRequest, ReplenishmentSuggestRequest, ScanCountRequest,
    SetReorderPointRequest, ShelfTagResponse, StartCountRequest, SuggestedLine,
    SuggestedPurchaseOrder, Transfer, TransferLineView,
};
pub use payments::{
    MockTerminal, PaymentError, PaymentProvider, PaymentRequest, PaymentResult, PaymentStatus,
    SurchargeRule, TenderKind, compute_surcharge, surcharge_rule_for,
};
pub use portal::{BopisOrderResponse, BopisOrderView, PortalCustomerView, PortalReorderResponse};
pub use scheduling::{
    ClockRequest, CreateShiftTemplateRequest, CreateStaffShiftRequest, SetShiftStatusRequest,
};
pub use tinting::{
    ColorantLevelView, ColorantsResponse, DispenseTintRequest, DispenseTintResponse,
    tint_job_to_row, virtual_tinter_from_levels,
};
pub use types::{
    CompletedSale, EdgeConfig, PricePreview, PricePreviewLine, ReturnLineInput, SaleInput,
    SaleLineInput, TenderInput, ZReport,
};
