//! Adversarial intent + matching coverage.
//!
//! PostIntent and the matcher are permissionless entry points. These
//! tests confirm malformed posts are rejected and the matcher refuses
//! economically invalid pairs.

use polyleverage::state::SIDE_LONG;
use polyleverage_sim::scenario::{SCENARIO_PRICE_FP, SCENARIO_EXPIRY_SLOT};
use polyleverage_sim::Scenario;
use solana_sdk::signature::Signer;

#[test]
fn post_intent_rejects_zero_contracts() {
    let mut s = Scenario::new();
    let res = s.h.post_intent(
        &s.long,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_LONG,
        SCENARIO_PRICE_FP,
        0,
        SCENARIO_EXPIRY_SLOT,
    );
    assert!(res.is_err(), "a zero-contract intent must be rejected");
}

#[test]
fn post_intent_rejects_invalid_side() {
    let mut s = Scenario::new();
    let res = s.h.post_intent(
        &s.long,
        &s.instrument,
        &s.book,
        &s.mint,
        7, // neither SIDE_LONG (0) nor SIDE_SHORT (1)
        SCENARIO_PRICE_FP,
        1,
        SCENARIO_EXPIRY_SLOT,
    );
    assert!(res.is_err(), "an out-of-range side byte must be rejected");
}

#[test]
fn post_intent_rejects_expired_expiration_slot() {
    let mut s = Scenario::new();
    // expiration_slot 0 is at or below the current slot → already expired.
    let res = s.h.post_intent(
        &s.long,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_LONG,
        SCENARIO_PRICE_FP,
        1,
        0,
    );
    assert!(res.is_err(), "an already-expired intent must be rejected");
}

#[test]
fn post_intent_rejects_zero_price() {
    let mut s = Scenario::new();
    // A limit price of 0 is outside the valid (0, 1) band.
    let res = s.h.post_intent(
        &s.long,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_LONG,
        0,
        1,
        SCENARIO_EXPIRY_SLOT,
    );
    assert!(res.is_err(), "a zero limit price must be rejected");
}

#[test]
fn post_intent_rejects_insufficient_collateral() {
    let mut s = Scenario::new();
    // A trader with a margin account but no deposit.
    let broke = s.h.create_user();
    s.h.create_margin_account(&broke, &s.mint);
    let res = s.h.post_intent(
        &broke,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_LONG,
        SCENARIO_PRICE_FP,
        1,
        SCENARIO_EXPIRY_SLOT,
    );
    assert!(
        res.is_err(),
        "posting without sufficient free collateral must be rejected"
    );
}

#[test]
fn match_rejects_non_opposite_sides() {
    let mut s = Scenario::new();

    // Two LONG intents.
    let id_a = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.long,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_LONG,
        SCENARIO_PRICE_FP,
        1,
        SCENARIO_EXPIRY_SLOT,
    )
    .expect("post long a");

    let id_b = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.short,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_LONG,
        SCENARIO_PRICE_FP,
        1,
        SCENARIO_EXPIRY_SLOT,
    )
    .expect("post long b");

    // Attempt to match them as if id_b were a short.
    let (res, _) = s.h.match_pair(
        &s.instrument,
        &s.book,
        &s.mint,
        &s.long.pubkey(),
        &s.short.pubkey(),
        id_a,
        id_b,
        &s.short.pubkey(),
        &s.long.pubkey(),
    );
    assert!(
        res.is_err(),
        "matching two same-side intents must be rejected"
    );
}
