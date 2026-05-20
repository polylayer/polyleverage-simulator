//! CreateInstrument then PostIntent long + short then MatchPair.
//!
//! Posts a crossing long and short, matches them, and verifies the
//! resulting PMLC. The rejection case: a long bidding below the short's
//! ask does not cross and must not match.

use polyleverage::state::{PMLC_STATUS_LIVE, SIDE_LONG, SIDE_SHORT};
use polyleverage_sim::scenario::{ENTRY_FP, SCENARIO_EXPIRY_SLOT};
use polyleverage_sim::Scenario;
use solana_sdk::signature::Signer;

#[test]
fn post_match_produces_live_pmlc() {
    let mut s = Scenario::new();
    let pmlc_pda = s.open_pmlc();

    let pmlc = s.h.load_pmlc(&pmlc_pda);
    assert_eq!(pmlc.status, PMLC_STATUS_LIVE, "PMLC must be live");
    assert_eq!(pmlc.long_owner, s.long.pubkey());
    assert_eq!(pmlc.short_owner, s.short.pubkey());
    assert_eq!(pmlc.instrument, s.instrument);
    assert_eq!(pmlc.collateral_mint, s.mint);
    assert_eq!(
        pmlc.collateral_per_side, s.params.collateral_bucket,
        "each side locks one collateral bucket"
    );
    assert_eq!(
        pmlc.entry_price_fp, ENTRY_FP,
        "entry settles at the crossing midpoint"
    );
}

#[test]
fn match_rejects_non_crossing_prices() {
    let mut s = Scenario::new();

    // Long bids 10, short asks 80 — the long will not pay the ask, so the
    // prices do not cross.
    let long_id = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.long,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_LONG,
        10,
        1,
        SCENARIO_EXPIRY_SLOT,
    )
    .expect("post long");

    let short_id = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.short,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_SHORT,
        80,
        1,
        SCENARIO_EXPIRY_SLOT,
    )
    .expect("post short");

    let (res, _) = s.h.match_pair(
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
    assert!(res.is_err(), "non-crossing prices must not match");
}
