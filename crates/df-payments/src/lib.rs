//! `df-payments` — payments for decor-pos (Phase 1, v0.2).
//!
//! Pluggable [`PaymentProvider`] trait using semi-integrated / tokenized
//! flows (spec §1, §6.2). A [`PaymentResult::reference`] is a provider
//! reference or token; [`PaymentResult::authorization`] is an approval
//! code string. This crate never stores, logs, or transmits raw card
//! data — there is no field for it anywhere in this workspace, and
//! `df-api` / `df-db` schemas contain no such columns (spec §8).
//!
//! * [`provider`] — [`PaymentProvider`], [`PaymentRequest`],
//!   [`PaymentResult`], [`TenderKind`], [`PaymentStatus`].
//! * [`surcharge`] — per-tender surcharge rules
//!   ([`compute_surcharge`]) and lookup ([`surcharge_rule_for`]).
//! * [`MockTerminal`] — in-memory provider for tests: approve-all,
//!   decline-all, scripted outcomes, idempotent replays.
//! * [`StandaloneTerminal`] — dumb external terminal: the cashier keys
//!   the amount in by hand; this crate only records reconciliation
//!   entries, with optional manual confirmation.
//! * [`StripeTerminal`] — real trait-implementation skeleton: reads
//!   credentials only from the environment, makes no live calls in this
//!   phase (those land in a later phase).
//!
//! Money is [`df_domain::Money`] throughout — zero floating-point.

pub mod error;
pub mod mock;
pub mod provider;
pub mod standalone;
pub mod stripe;
pub mod surcharge;

pub use error::PaymentError;
pub use mock::{MockOutcome, MockTerminal};
pub use provider::{PaymentProvider, PaymentRequest, PaymentResult, PaymentStatus, TenderKind};
pub use standalone::{ReconciliationRecord, StandaloneConfig, StandaloneTerminal};
pub use stripe::StripeTerminal;
pub use surcharge::{SurchargeRule, compute_surcharge, surcharge_rule_for};
