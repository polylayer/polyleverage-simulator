//! Adversarial coverage of the single-price matching engine.
//!
//! Each test drives a full transaction through litesvm and probes one
//! property of `match_pair_core`: the cross rule, side validation,
//! expiry, double-spend of a filled intent, midpoint settlement, and
//! best-bid/best-ask discovery via `MatchBestAvailable`.

use polyleverage::state::{PMLC_STATUS_LIVE, SIDE_LONG, SIDE_SHORT};
use polyleverage_sim::scenario::SCENARIO_EXPIRY_SLOT;
use polyleverage_sim::Scenario;
use solana_sdk::signature::Signer;

/// A keeper naming two same-side intents as a pair must be rejected:
/// `find_intent_by_id` resolves both to the long tree, so the pair is
/// not opposite-sided.
#[test]
fn match_rejects_same_side_pair() {
    let mut s = Scenario::new();
    let other = s.new_funded_trader();

    let id_a = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.long, &s.instrument, &s.book, &s.mint, SIDE_LONG, 40, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post long a");

    let id_b = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &other, &s.instrument, &s.book, &s.mint, SIDE_LONG, 40, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post long b");

    // Pass two longs as (long_id, short_id).
    let (res, _) = s.h.match_pair(
        &s.instrument, &s.book, &s.mint, &s.long.pubkey(), &other.pubkey(), id_a, id_b,
        &other.pubkey(), &s.long.pubkey(),
    );
    assert!(res.is_err(), "two same-side intents must not match");
}

/// Two intents at the same price cross, and the entry settles exactly
/// there (the midpoint of equal prices is the price).
#[test]
fn equal_price_crosses_at_that_price() {
    let mut s = Scenario::new();
    let pmlc = s.open_pmlc_at(30);
    let p = s.h.load_pmlc(&pmlc);
    assert_eq!(p.status, PMLC_STATUS_LIVE);
    assert_eq!(p.entry_price_fp, 30, "equal-price cross settles at the price");
}

/// A long bidding above the short's ask crosses, and the entry is the
/// midpoint of the two limit prices — the spread is split evenly.
#[test]
fn crossing_spread_settles_at_midpoint() {
    let mut s = Scenario::new();

    let long_id = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.long, &s.instrument, &s.book, &s.mint, SIDE_LONG, 70, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post long");

    let short_id = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.short, &s.instrument, &s.book, &s.mint, SIDE_SHORT, 30, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post short");

    let (res, pmlc) = s.h.match_pair(
        &s.instrument, &s.book, &s.mint, &s.long.pubkey(), &s.short.pubkey(), long_id,
        short_id, &s.short.pubkey(), &s.long.pubkey(),
    );
    res.expect("crossing pair must match");
    assert_eq!(
        s.h.load_pmlc(&pmlc).entry_price_fp,
        50,
        "entry is the midpoint of 70 and 30"
    );
}

/// Once an intent is fully filled its node is freed, so a second
/// `MatchPair` naming the same ids must fail to resolve them.
#[test]
fn match_rejects_already_filled_intent() {
    let mut s = Scenario::new();

    let long_id = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.long, &s.instrument, &s.book, &s.mint, SIDE_LONG, 50, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post long");
    let short_id = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.short, &s.instrument, &s.book, &s.mint, SIDE_SHORT, 50, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post short");

    let (first, _) = s.h.match_pair(
        &s.instrument, &s.book, &s.mint, &s.long.pubkey(), &s.short.pubkey(), long_id,
        short_id, &s.short.pubkey(), &s.long.pubkey(),
    );
    first.expect("first match succeeds");

    let (second, _) = s.h.match_pair(
        &s.instrument, &s.book, &s.mint, &s.long.pubkey(), &s.short.pubkey(), long_id,
        short_id, &s.short.pubkey(), &s.long.pubkey(),
    );
    assert!(second.is_err(), "a filled intent cannot be matched again");
}

/// An intent matched after its expiration slot must be rejected.
#[test]
fn match_rejects_expired_intent() {
    let mut s = Scenario::new();

    let long_id = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.long, &s.instrument, &s.book, &s.mint, SIDE_LONG, 50, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post long");
    let short_id = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.short, &s.instrument, &s.book, &s.mint, SIDE_SHORT, 50, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post short");

    // Advance past the intents' expiration slot.
    s.h.warp_slot(SCENARIO_EXPIRY_SLOT + 1);

    let (res, _) = s.h.match_pair(
        &s.instrument, &s.book, &s.mint, &s.long.pubkey(), &s.short.pubkey(), long_id,
        short_id, &s.short.pubkey(), &s.long.pubkey(),
    );
    assert!(res.is_err(), "an expired intent must not match");
}

/// `MatchBestAvailable` must pick the highest bid against the lowest
/// ask, ignoring the worse-priced orders on each side.
#[test]
fn match_best_picks_best_bid_and_ask() {
    let mut s = Scenario::new();
    let long_hi = s.new_funded_trader();
    let short_lo = s.new_funded_trader();

    // Worse long (40) and worse short (50); better long (60), better short (30).
    s.h.post_intent(
        &s.long, &s.instrument, &s.book, &s.mint, SIDE_LONG, 40, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post worse long");
    s.h.post_intent(
        &long_hi, &s.instrument, &s.book, &s.mint, SIDE_LONG, 60, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post best long");
    s.h.post_intent(
        &s.short, &s.instrument, &s.book, &s.mint, SIDE_SHORT, 50, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post worse short");
    s.h.post_intent(
        &short_lo, &s.instrument, &s.book, &s.mint, SIDE_SHORT, 30, 1, SCENARIO_EXPIRY_SLOT,
    )
    .expect("post best short");

    // short_lo posted last → taker; long_hi → maker.
    let (res, pmlc) = s.h.match_best(
        &s.instrument, &s.book, &s.mint, &long_hi.pubkey(), &short_lo.pubkey(),
        &short_lo.pubkey(), &long_hi.pubkey(),
    );
    res.expect("a crossing pair exists");

    let p = s.h.load_pmlc(&pmlc);
    assert_eq!(p.long_owner, long_hi.pubkey(), "best bid (60) is matched");
    assert_eq!(p.short_owner, short_lo.pubkey(), "best ask (30) is matched");
    assert_eq!(p.entry_price_fp, 45, "entry is the midpoint of 60 and 30");
}
