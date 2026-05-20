//! Compute-unit benchmarks.
//!
//! Runs each meaningful instruction with representative inputs and
//! records the compute units consumed (litesvm reports per-transaction
//! CU). Prints a table — run with `cargo test --test benchmarks --
//! --nocapture` — and asserts every instruction stays well under the
//! 200k per-instruction limit, so a CU regression fails the suite.
//!
//! The committed numbers live in `BENCHMARKS.md`.

use polyleverage::state::{SIDE_LONG, SIDE_SHORT};
use polyleverage_sim::driver::TxResult;
use polyleverage_sim::scenario::{RANGE_MAX_FP, RANGE_MIN_FP, SCENARIO_EXPIRY_SLOT};
use polyleverage_sim::Scenario;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

/// Per-instruction CU ceiling — the Solana default single-instruction
/// limit. Any instruction approaching this needs attention.
const CU_CEILING: u64 = 200_000;

fn cu(r: TxResult) -> u64 {
    r.expect("transaction succeeded").compute_units_consumed
}

#[test]
fn compute_unit_benchmarks() {
    let mut s = Scenario::new();
    let mut rows: Vec<(&str, u64)> = Vec::new();

    // --- Deposit / Withdraw ---
    let trader = s.h.create_user();
    let ata = Pubkey::new_unique();
    s.h.create_token_account(&ata, &s.mint, &trader.pubkey(), 10_000_000);
    s.h.create_margin_account(&trader, &s.mint);
    rows.push(("Deposit", cu(s.h.deposit(&trader, &s.mint, &ata, 5_000_000))));
    rows.push((
        "Withdraw",
        cu(s.h.withdraw(&trader, &s.mint, &ata, 2_000_000)),
    ));

    // --- PostIntent (long + short) ---
    let long_id = s.h.book_next_intent_id(&s.book);
    rows.push((
        "PostIntent (long)",
        cu(s.h.post_intent(
            &s.long,
            &s.instrument,
            &s.book,
            &s.mint,
            SIDE_LONG,
            RANGE_MIN_FP,
            RANGE_MAX_FP,
            1,
            SCENARIO_EXPIRY_SLOT,
        )),
    ));
    let short_id = s.h.book_next_intent_id(&s.book);
    rows.push((
        "PostIntent (short)",
        cu(s.h.post_intent(
            &s.short,
            &s.instrument,
            &s.book,
            &s.mint,
            SIDE_SHORT,
            RANGE_MIN_FP,
            RANGE_MAX_FP,
            1,
            SCENARIO_EXPIRY_SLOT,
        )),
    ));

    // --- MatchPair → PMLC ---
    let (match_res, pmlc1) = s.h.match_pair(
        &s.instrument,
        &s.book,
        &s.mint,
        &s.long.pubkey(),
        &s.short.pubkey(),
        long_id,
        short_id,
        &s.short.pubkey(),
        &s.long.pubkey(),
    );
    rows.push(("MatchPair", cu(match_res)));

    // --- Novate ---
    let recipient = s.new_funded_trader();
    rows.push((
        "Novate",
        cu(s.h.novate(
            &s.instrument,
            &pmlc1,
            &s.mint,
            SIDE_LONG,
            &s.long,
            &recipient,
        )),
    ));

    // --- Liquidate (incl. Ed25519 attestation verify) ---
    let pmlc2 = s.open_pmlc();
    let keeper = s.new_funded_trader();
    rows.push((
        "Liquidate (+attestation)",
        cu(s.h.liquidate(
            &s.attestor,
            &s.instrument,
            &pmlc2,
            &s.mint,
            s.params.market_id,
            &keeper,
            &s.long.pubkey(),
            &s.short.pubkey(),
            0,
            1,
        )),
    ));

    // --- Resolve (incl. Ed25519 attestation verify) ---
    let pmlc3 = s.open_pmlc();
    rows.push((
        "Resolve (+attestation)",
        cu(s.h.resolve(
            &s.attestor,
            &s.instrument,
            &pmlc3,
            &s.mint,
            s.params.market_id,
            &s.long.pubkey(),
            &s.short.pubkey(),
            10_000,
            2,
        )),
    ));

    // --- ClosePmlc ---
    rows.push(("ClosePmlc", cu(s.h.close_pmlc(&pmlc3))));

    println!("\n  polyleverage compute-unit benchmarks (litesvm)");
    println!("  {}", "-".repeat(46));
    for (name, consumed) in &rows {
        println!("  {name:<30} {consumed:>8} CU");
        assert!(
            *consumed < CU_CEILING,
            "{name} consumed {consumed} CU — over the {CU_CEILING} ceiling"
        );
    }
    println!("  {}", "-".repeat(46));
}
