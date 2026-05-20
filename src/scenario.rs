//! High-level scenario builder.
//!
//! Every settlement test needs the same substrate: a configured
//! program, a fee schedule, a collateral mint + vault, an instrument,
//! and two funded traders. `Scenario` assembles all of it and can
//! drive a long + short pair into a live PMLC, so individual tests
//! only express what they are actually testing.

use solana_sdk::{pubkey::Pubkey, signature::Keypair, signature::Signer};

use polyleverage::state::{SIDE_LONG, SIDE_SHORT};

use crate::attestor::Attestor;
use crate::driver::{Harness, InstrumentParams};

/// Intent expiration slot used across scenario posts — far enough out
/// that nothing expires mid-test.
pub const SCENARIO_EXPIRY_SLOT: u64 = 1_000_000;

/// Overlapping price range both scenario intents post over. Entry price
/// settles at the overlap midpoint (50). These are normalized prices in
/// the program's `(0, 1)` fixed-point.
pub const RANGE_MIN_FP: u64 = 40;
pub const RANGE_MAX_FP: u64 = 60;
pub const ENTRY_FP: u64 = 50;

/// Default per-trader deposit floor. The actual deposit scales up with
/// the instrument's collateral bucket (see `Scenario::with_params`).
pub const TRADER_DEPOSIT: u64 = 50_000_000;

/// A fully wired single-instrument market with two funded traders.
pub struct Scenario {
    pub h: Harness,
    pub attestor: Attestor,
    pub params: InstrumentParams,
    /// The collateral each trader deposited (≥ a few collateral buckets).
    pub trader_deposit: u64,
    pub mint: Pubkey,
    pub vault: Pubkey,
    pub instrument: Pubkey,
    pub book: Pubkey,
    pub long: Keypair,
    pub short: Keypair,
}

impl Default for Scenario {
    fn default() -> Self {
        Self::new()
    }
}

impl Scenario {
    /// Build the full substrate with the default instrument parameters.
    pub fn new() -> Self {
        Self::with_params(InstrumentParams::default())
    }

    /// Build the full substrate for a given instrument parameter set:
    /// program config, fee schedule, mint + vault, one instrument, two
    /// traders each with a margin account and a deposit. The deposit
    /// scales with the collateral bucket so high-bucket instruments are
    /// still fundable.
    pub fn with_params(params: InstrumentParams) -> Self {
        let mut h = Harness::new();
        let attestor = Attestor::new();
        h.init_program_config(attestor.signer_bytes(), 300);
        h.init_fee_schedule();

        let mint = h.create_mint(6);
        let vault = h.init_vault_ata(&mint);

        let (instrument, book) = h.create_instrument(&mint, &params);

        // Enough for several PMLCs at this instrument's bucket size.
        let trader_deposit = TRADER_DEPOSIT.max(params.collateral_bucket.saturating_mul(8));
        let long = Self::funded_trader(&mut h, &mint, trader_deposit);
        let short = Self::funded_trader(&mut h, &mint, trader_deposit);

        Self {
            h,
            attestor,
            params,
            trader_deposit,
            mint,
            vault,
            instrument,
            book,
            long,
            short,
        }
    }

    /// Add another funded trader (SOL, token account, margin account, and
    /// a deposit) to the market — used by tests that need a third party
    /// (novation recipient, substitution counterparty, liquidation keeper).
    pub fn new_funded_trader(&mut self) -> Keypair {
        Self::funded_trader(&mut self.h, &self.mint, self.trader_deposit)
    }

    /// Create a trader: a SOL-funded keypair, a token account holding
    /// collateral, a margin account, and a deposit of `deposit` atoms.
    fn funded_trader(h: &mut Harness, mint: &Pubkey, deposit: u64) -> Keypair {
        let trader = h.create_user();
        let ata = Pubkey::new_unique();
        h.create_token_account(&ata, mint, &trader.pubkey(), deposit.saturating_mul(2));
        h.create_margin_account(&trader, mint);
        h.deposit(&trader, mint, &ata, deposit)
            .expect("trader deposit");
        trader
    }

    /// Post an overlapping long + short over the default scenario range
    /// and match them into a live PMLC. Returns the PMLC PDA.
    pub fn open_pmlc(&mut self) -> Pubkey {
        self.open_pmlc_at(RANGE_MIN_FP, RANGE_MAX_FP)
    }

    /// Post a long + short over `[min_price_fp, max_price_fp]` and match
    /// them into a live PMLC. The entry price settles at the overlap
    /// midpoint; pass `min == max` for an exact entry. Used by the
    /// multi-asset tests to open positions at a normalized Pyth price.
    /// Panics if any step fails.
    pub fn open_pmlc_at(&mut self, min_price_fp: u64, max_price_fp: u64) -> Pubkey {
        let long_id = self.h.book_next_intent_id(&self.book);
        self.h
            .post_intent(
                &self.long,
                &self.instrument,
                &self.book,
                &self.mint,
                SIDE_LONG,
                min_price_fp,
                max_price_fp,
                1,
                SCENARIO_EXPIRY_SLOT,
            )
            .expect("post long intent");

        let short_id = self.h.book_next_intent_id(&self.book);
        self.h
            .post_intent(
                &self.short,
                &self.instrument,
                &self.book,
                &self.mint,
                SIDE_SHORT,
                min_price_fp,
                max_price_fp,
                1,
                SCENARIO_EXPIRY_SLOT,
            )
            .expect("post short intent");

        // Short was posted last → it is the taker.
        let (res, pmlc) = self.h.match_pair(
            &self.instrument,
            &self.book,
            &self.mint,
            &self.long.pubkey(),
            &self.short.pubkey(),
            long_id,
            short_id,
            &self.short.pubkey(),
            &self.long.pubkey(),
        );
        res.expect("match pair");
        pmlc
    }
}
