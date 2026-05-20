//! Adversarial attestation coverage.
//!
//! Settlement (Liquidate / Resolve) is the protocol's trust boundary:
//! it moves locked collateral on the strength of a TEE-signed
//! attestation. These tests confirm every forgery vector is rejected —
//! wrong signer, wrong type, wrong PMLC binding, replayed nonce, wrong
//! market, and a missing attestation instruction entirely.
//!
//! Each test differs from a known-good settlement in exactly one way,
//! so a rejection isolates the vector under test.

use polyleverage_sim::{Attestor, Scenario};
use solana_sdk::signature::Signer;

#[test]
fn liquidate_rejects_forged_signer() {
    let mut s = Scenario::new();
    let pmlc = s.open_pmlc();
    let keeper = s.new_funded_trader();

    // An attestor whose key is NOT the registered attestation signer.
    let forged = Attestor::new();
    let now = s.h.now_unix();
    let att = forged.historical_liquidation(
        s.params.market_id,
        now.max(0) as u64,
        1,
        pmlc.to_bytes(),
        0,
        now,
    );
    let res = s.h.liquidate_signed(
        &forged,
        &att,
        &s.instrument,
        &pmlc,
        &s.mint,
        s.params.market_id,
        &keeper,
        &s.long.pubkey(),
        &s.short.pubkey(),
    );
    assert!(
        res.is_err(),
        "an attestation signed by an unregistered key must be rejected"
    );
    assert_eq!(s.h.load_pmlc(&pmlc).status, 0, "PMLC must remain live");
}

#[test]
fn liquidate_rejects_wrong_attestation_type() {
    let mut s = Scenario::new();
    let pmlc = s.open_pmlc();
    let keeper = s.new_funded_trader();

    // A PRICE_TWAP attestation, validly signed by the real attestor —
    // but Liquidate requires a HISTORICAL_LIQUIDATION attestation.
    let now = s.h.now_unix();
    let att = s
        .attestor
        .price_twap(s.params.market_id, now.max(0) as u64, 1, 50, 100);
    let res = s.h.liquidate_signed(
        &s.attestor,
        &att,
        &s.instrument,
        &pmlc,
        &s.mint,
        s.params.market_id,
        &keeper,
        &s.long.pubkey(),
        &s.short.pubkey(),
    );
    assert!(
        res.is_err(),
        "a PRICE_TWAP attestation must not settle a Liquidate"
    );
}

#[test]
fn liquidate_rejects_attestation_bound_to_other_pmlc() {
    let mut s = Scenario::new();
    let p1 = s.open_pmlc();
    let p2 = s.open_pmlc();
    let keeper = s.new_funded_trader();

    // Attestation binds to p1's pubkey; we submit it against p2.
    let now = s.h.now_unix();
    let att = s.attestor.historical_liquidation(
        s.params.market_id,
        now.max(0) as u64,
        1,
        p1.to_bytes(),
        0,
        now,
    );
    let res = s.h.liquidate_signed(
        &s.attestor,
        &att,
        &s.instrument,
        &p2,
        &s.mint,
        s.params.market_id,
        &keeper,
        &s.long.pubkey(),
        &s.short.pubkey(),
    );
    assert!(
        res.is_err(),
        "an attestation bound to a different PMLC must be rejected"
    );
    assert_eq!(s.h.load_pmlc(&p2).status, 0, "target PMLC must remain live");
}

#[test]
fn liquidate_rejects_replayed_nonce() {
    let mut s = Scenario::new();
    let p1 = s.open_pmlc();
    let p2 = s.open_pmlc();
    let keeper = s.new_funded_trader();

    // First liquidation consumes nonce 5 on this market.
    s.h.liquidate(
        &s.attestor,
        &s.instrument,
        &p1,
        &s.mint,
        s.params.market_id,
        &keeper,
        &s.long.pubkey(),
        &s.short.pubkey(),
        0,
        5,
    )
    .expect("first liquidation");

    // A second liquidation on the same market replays nonce 5.
    let res = s.h.liquidate(
        &s.attestor,
        &s.instrument,
        &p2,
        &s.mint,
        s.params.market_id,
        &keeper,
        &s.long.pubkey(),
        &s.short.pubkey(),
        0,
        5,
    );
    assert!(
        res.is_err(),
        "replaying a consumed attestation nonce must be rejected"
    );
}

#[test]
fn liquidate_rejects_wrong_market_id() {
    let mut s = Scenario::new();
    let pmlc = s.open_pmlc();
    let keeper = s.new_funded_trader();

    // Attestation carries a market_id that is not this instrument's.
    let now = s.h.now_unix();
    let att = s.attestor.historical_liquidation(
        [0xCDu8; 32],
        now.max(0) as u64,
        1,
        pmlc.to_bytes(),
        0,
        now,
    );
    let res = s.h.liquidate_signed(
        &s.attestor,
        &att,
        &s.instrument,
        &pmlc,
        &s.mint,
        s.params.market_id,
        &keeper,
        &s.long.pubkey(),
        &s.short.pubkey(),
    );
    assert!(
        res.is_err(),
        "an attestation for a different market must be rejected"
    );
}

#[test]
fn liquidate_rejects_missing_attestation_instruction() {
    let mut s = Scenario::new();
    let pmlc = s.open_pmlc();
    let keeper = s.new_funded_trader();

    // Submit the Liquidate instruction with no preceding Ed25519 ix.
    let ix = s.h.liquidate_ix(
        &s.instrument,
        &pmlc,
        &s.mint,
        s.params.market_id,
        &keeper.pubkey(),
        &s.long.pubkey(),
        &s.short.pubkey(),
    );
    let res = s.h.send(&[ix], &[&keeper]);
    assert!(
        res.is_err(),
        "Liquidate without an attestation instruction must be rejected"
    );
}

#[test]
fn resolve_rejects_forged_signer() {
    let mut s = Scenario::new();
    let pmlc = s.open_pmlc();

    let forged = Attestor::new();
    let now = s.h.now_unix();
    let att = forged.resolution(
        s.params.market_id,
        now.max(0) as u64,
        1,
        10_000,
        now.max(0) as u64,
    );
    let res = s.h.resolve_signed(
        &forged,
        &att,
        &s.instrument,
        &pmlc,
        &s.mint,
        s.params.market_id,
        &s.long.pubkey(),
        &s.short.pubkey(),
    );
    assert!(
        res.is_err(),
        "a resolution signed by an unregistered key must be rejected"
    );
    assert_eq!(s.h.load_pmlc(&pmlc).status, 0, "PMLC must remain live");
}
