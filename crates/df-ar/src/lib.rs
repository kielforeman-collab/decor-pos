//! `df-ar` — contractor AR domain (Phase 3, v0.4; spec §4
//! `contractor_ar`, §5.4 `ar_ledger` / `ar_allocations`).
//!
//! Pure domain logic, no I/O: charge accounts and credit checks,
//! open-entry derivation from the append-only ledger, FIFO payment
//! allocation, finance charges, the collections queue and the
//! statement-batch data model. Persistence lives in `df-db` (`ar`
//! module); the edge daemon wires it into checkout in `df-edge`.
//!
//! All money is [`Money`] (`rust_decimal::Decimal`, scale 4) —
//! never floats. Balances are always derived from ledger entries,
//! never stored.
//!
//! Two rules callers must know:
//!
//! * **Credit limit is a hard ceiling.** A charge is rejected when
//!   the resulting balance would reach *or exceed* the credit limit
//!   (`open + new >= limit`). A limit of zero means no credit has
//!   been extended: any positive charge is rejected.
//! * **Finance charges are assessed once per statement period**
//!   (calendar month). The assessment is keyed by a deterministic
//!   period UUID ([`statement_period_key`]); re-running the same
//!   period finds the existing `finance_charge` ledger entry and
//!   posts nothing.

pub mod account;
pub mod collections;
pub mod finance;
pub mod ledger;
pub mod statement;

pub use account::{AccountRow, ChargeAccount, CreditError, Terms, account_from_row, parse_terms};
pub use collections::{CollectionsEntry, collections_queue, next_action_for};
pub use finance::{
    FinanceChargePosted, finance_charge_for, overdue_balance, period_for, statement_period_key,
};
pub use ledger::{Allocation, LedgerEntry, OpenCharge, allocate_payment, open_charges};
pub use statement::{AgingSummary, Statement, StatementLine};
