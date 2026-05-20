//! Compute-unit benchmarks.
//!
//! `compute_unit_benchmarks` runs each instruction with representative
//! inputs and records the compute units consumed. `compute_unit_scaling`
//! measures how PostIntent and MatchPair grow with the book's node
//! capacity, since the matching and prune paths scan the node pool.
//! Run with `cargo test --test benchmarks -- --nocapture`. The
//! committed numbers live in `BENCHMARKS.md`.

use polyleverage::state::{SIDE_LONG, SIDE_SHORT};
use polyleverage_sim::driver::TxResult;
use polyleverage_sim::scenario::{SCENARIO_PRICE_FP, SCENARIO_EXPIRY_SLOT};
use polyleverage_sim::{Attestor, Harness, InstrumentParams, Scenario};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};

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
            SCENARIO_PRICE_FP,
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
            SCENARIO_PRICE_FP,
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

// --- Scaling: compute units vs book capacity --------------------------------

/// Create a SOL-funded trader with a margin account and a deposit.
fn fund_trader(h: &mut Harness, mint: &Pubkey) -> Keypair {
    let t = h.create_user();
    let ata = Pubkey::new_unique();
    h.create_token_account(&ata, mint, &t.pubkey(), 1_000_000);
    h.create_margin_account(&t, mint);
    h.deposit(&t, mint, &ata, 1_000_000).expect("trader deposit");
    t
}

/// Measure PostIntent and MatchPair compute units against a book of a
/// given node capacity. The matching, cancel, and prune paths scan the
/// whole node pool, so cost tracks the book's provisioned capacity, not
/// its live fill — the book here holds only a few intents. Returns
/// `None` for a leg whose transaction exceeded the 1.4M limit.
fn measure_scaling(capacity: u32) -> (Option<u64>, Option<u64>) {
    let mut h = Harness::new();
    h.init_program_config(Attestor::new().signer_bytes(), 300);
    h.init_fee_schedule();
    let mint = h.create_mint(6);
    h.init_vault_ata(&mint);

    // The book account can only be created small: Solana caps account
    // growth at ~10 KiB per transaction, so a deep book is created at a
    // minimal capacity and grown with repeated ExpandIntentBook calls.
    let params = InstrumentParams {
        collateral_bucket: 1_000,
        initial_book_capacity: 16,
        ..InstrumentParams::default()
    };
    let (instrument, book) = h.create_instrument(&mint, &params);
    while h.book_capacity(&book) < capacity {
        let remaining = capacity - h.book_capacity(&book);
        h.expand_intent_book(&instrument, &book, remaining.min(100))
            .expect("expand book");
    }

    let a = fund_trader(&mut h, &mint);
    let b = fund_trader(&mut h, &mint);

    // A's first post creates A's seat. prune-on-post is skipped on a
    // trader's first post, so this one is not the measurement.
    let long_id = h.book_next_intent_id(&book);
    h.post_intent(
        &a, &instrument, &book, &mint, SIDE_LONG, SCENARIO_PRICE_FP, 1,
        SCENARIO_EXPIRY_SLOT,
    )
    .expect("seed long");

    // A's second post is measured: A now has a seat, so prune-on-post
    // runs its O(capacity) scan over the node pool.
    let post_ix = h.post_intent_ix(
        &a, &instrument, &book, &mint, SIDE_LONG, SCENARIO_PRICE_FP, 1,
        SCENARIO_EXPIRY_SLOT,
    );
    let post_cu = h
        .send_metered(&[post_ix], &[&a], 1_400_000)
        .ok()
        .map(|m| m.compute_units_consumed);

    // A short to match against.
    let short_id = h.book_next_intent_id(&book);
    h.post_intent(
        &b, &instrument, &book, &mint, SIDE_SHORT, SCENARIO_PRICE_FP, 1,
        SCENARIO_EXPIRY_SLOT,
    )
    .expect("seed short");

    // MatchPair is measured: find_intent_by_id scans the pool twice.
    let (match_ix, _) = h.match_pair_ix(
        &instrument, &book, &mint, &a.pubkey(), &b.pubkey(), long_id, short_id,
        &b.pubkey(), &a.pubkey(),
    );
    let match_cu = h
        .send_metered(&[match_ix], &[], 1_400_000)
        .ok()
        .map(|m| m.compute_units_consumed);

    (post_cu, match_cu)
}

#[test]
fn compute_unit_scaling() {
    let caps = [16u32, 64, 256, 1024, 4096, 8192];
    let rows: Vec<(u32, Option<u64>, Option<u64>)> =
        caps.iter().map(|&c| {
            let (p, m) = measure_scaling(c);
            (c, p, m)
        }).collect();

    let fmt = |v: Option<u64>| {
        v.map(|x| x.to_string()).unwrap_or_else(|| "over 1.4M".into())
    };
    println!("\n  polyleverage compute units vs book capacity (litesvm)");
    println!("  {}", "-".repeat(54));
    println!("  {:>10}  {:>16}  {:>16}", "capacity", "PostIntent CU", "MatchPair CU");
    println!("  {}", "-".repeat(54));
    for (c, p, m) in &rows {
        println!("  {c:>10}  {:>16}  {:>16}", fmt(*p), fmt(*m));
    }
    println!("  {}", "-".repeat(54));

    // Linear fit on the measured MatchPair points; extrapolate the
    // capacity at which it crosses the 200k default and 1.4M maximum.
    let pts: Vec<(f64, f64)> = rows
        .iter()
        .filter_map(|(c, _, m)| m.map(|v| (*c as f64, v as f64)))
        .collect();
    if pts.len() >= 2 {
        let (c0, m0) = pts[0];
        let (c1, m1) = *pts.last().unwrap();
        let slope = (m1 - m0) / (c1 - c0);
        let base = m0 - slope * c0;
        println!("  MatchPair fit: ~{:.0} CU + ~{:.1} CU per node", base, slope);
        println!(
            "  crosses 200k default limit at capacity ~{:.0}",
            (200_000.0 - base) / slope
        );
        println!(
            "  crosses 1.4M maximum limit at capacity ~{:.0}",
            (1_400_000.0 - base) / slope
        );
    }
    println!("  {}", "-".repeat(54));

    assert!(
        rows[0].2.map_or(false, |m| m < CU_CEILING),
        "MatchPair at capacity 16 should be under the 200k default"
    );
}
