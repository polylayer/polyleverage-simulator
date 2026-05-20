//! Settlement: liquidate, resolve, close.
//!
//! These exercise the attestation-gated terminal paths. Each submits a
//! TEE-signed attestation (the harness `Attestor`) alongside the
//! settlement instruction. They require the per-market `MarketNonce`
//! account, which `CreateInstrument` creates.

use polyleverage::state::{
    CLOSE_REASON_LIQUIDATED, CLOSE_REASON_RESOLVED, PMLC_STATUS_LIQUIDATED, PMLC_STATUS_RESOLVED,
};
use polyleverage_sim::scenario::ENTRY_FP;
use polyleverage_sim::Scenario;
use solana_sdk::signature::Signer;

// --- Liquidate ----------------------------------------------------------

#[test]
fn liquidate_settles_underwater_position_and_pays_bounty() {
    let mut s = Scenario::new();
    let pmlc_pda = s.open_pmlc();
    let keeper = s.new_funded_trader();
    let bucket = s.params.collateral_bucket;

    // Breach mark of 0 collapses the long's equity → liquidatable.
    s.h.liquidate(
        &s.attestor,
        &s.instrument,
        &pmlc_pda,
        &s.mint,
        s.params.market_id,
        &keeper,
        &s.long.pubkey(),
        &s.short.pubkey(),
        0,
        1,
    )
    .expect("liquidate underwater position");

    let pmlc = s.h.load_pmlc(&pmlc_pda);
    assert_eq!(pmlc.status, PMLC_STATUS_LIQUIDATED);
    assert_eq!(pmlc.close_reason, CLOSE_REASON_LIQUIDATED);

    // Bounty = bucket * liquidation_bounty_bps / 10_000 = 1e6 * 100 / 1e4.
    let bounty = bucket * s.params.liquidation_bounty_bps as u64 / 10_000;
    let keeper_margin = s.h.load_margin(&keeper.pubkey(), &s.mint);
    assert_eq!(
        keeper_margin.free_collateral,
        s.trader_deposit + bounty,
        "keeper margin credited the liquidation bounty"
    );

    // Winner (short) collected 2c - bounty; loser (long) got nothing back.
    let short_margin = s.h.load_margin(&s.short.pubkey(), &s.mint);
    assert_eq!(
        short_margin.free_collateral,
        (s.trader_deposit - bucket) + (2 * bucket - bounty)
    );
    assert_eq!(short_margin.locked_collateral, 0);
}

#[test]
fn liquidate_rejects_healthy_position() {
    let mut s = Scenario::new();
    let pmlc_pda = s.open_pmlc();
    let keeper = s.new_funded_trader();

    // Breach mark equal to the entry price → both sides hold full
    // collateral → not liquidatable.
    let res = s.h.liquidate(
        &s.attestor,
        &s.instrument,
        &pmlc_pda,
        &s.mint,
        s.params.market_id,
        &keeper,
        &s.long.pubkey(),
        &s.short.pubkey(),
        ENTRY_FP,
        1,
    );
    assert!(res.is_err(), "a healthy position must not be liquidatable");
    assert_eq!(s.h.load_pmlc(&pmlc_pda).status, 0, "PMLC still live");
}

// --- Resolve ------------------------------------------------------------

#[test]
fn resolve_marks_pmlc_resolved() {
    let mut s = Scenario::new();
    let pmlc_pda = s.open_pmlc();
    let bucket = s.params.collateral_bucket;

    // Outcome 10000 → long wins fully.
    s.h.resolve(
        &s.attestor,
        &s.instrument,
        &pmlc_pda,
        &s.mint,
        s.params.market_id,
        &s.long.pubkey(),
        &s.short.pubkey(),
        10_000,
        1,
    )
    .expect("resolve market");

    let pmlc = s.h.load_pmlc(&pmlc_pda);
    assert_eq!(pmlc.status, PMLC_STATUS_RESOLVED);
    assert_eq!(pmlc.close_reason, CLOSE_REASON_RESOLVED);

    // Long collects 2c equity; short collects 0. Each had `bucket`
    // locked, so long's free balance rises by 2c.
    let long_margin = s.h.load_margin(&s.long.pubkey(), &s.mint);
    assert_eq!(
        long_margin.free_collateral,
        (s.trader_deposit - bucket) + 2 * bucket
    );
    assert_eq!(long_margin.locked_collateral, 0);
}

#[test]
fn resolve_rejects_invalid_outcome() {
    let mut s = Scenario::new();
    let pmlc_pda = s.open_pmlc();

    // Only 0 / 5000 / 10000 are valid outcomes.
    let res = s.h.resolve(
        &s.attestor,
        &s.instrument,
        &pmlc_pda,
        &s.mint,
        s.params.market_id,
        &s.long.pubkey(),
        &s.short.pubkey(),
        3_000,
        1,
    );
    assert!(res.is_err(), "a non-canonical outcome must be rejected");
}

// --- ClosePmlc ----------------------------------------------------------

#[test]
fn close_pmlc_after_resolution_refunds_rent() {
    let mut s = Scenario::new();
    let pmlc_pda = s.open_pmlc();
    s.h.resolve(
        &s.attestor,
        &s.instrument,
        &pmlc_pda,
        &s.mint,
        s.params.market_id,
        &s.long.pubkey(),
        &s.short.pubkey(),
        10_000,
        1,
    )
    .expect("resolve");

    assert!(
        s.h.account(&pmlc_pda).is_some(),
        "PMLC account exists before close"
    );

    s.h.close_pmlc(&pmlc_pda).expect("close resolved PMLC");

    assert!(
        s.h.account(&pmlc_pda).map_or(true, |a| a.lamports == 0),
        "closed PMLC account must be emptied of rent"
    );
}

#[test]
fn close_pmlc_rejects_live_pmlc() {
    let mut s = Scenario::new();
    let pmlc_pda = s.open_pmlc();

    // A live PMLC is not closeable.
    assert!(
        s.h.close_pmlc(&pmlc_pda).is_err(),
        "a live PMLC must not be closeable"
    );
}
