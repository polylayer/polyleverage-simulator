//! Local simulation harness for the polyleverage Solana program.
//!
//! Loads the prebuilt SBF artifact into an in-process SVM (litesvm)
//! and drives it end-to-end with a simulated CRE/TEE attestor. The
//! `tests/` directory holds the E2E suite; `pyth_feed.py` supplies
//! real or mocked oracle prices.
//!
//! See `README.md` for usage.

pub mod attestor;
pub mod driver;
pub mod pricing;
pub mod scenario;

pub use attestor::Attestor;
pub use driver::{Harness, InstrumentParams, SOL};
pub use pricing::normalize_price;
pub use scenario::Scenario;
