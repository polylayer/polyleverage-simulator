//! MatchSubstituteWithSettle.
//!
//! The original long owner exits a live PMLC against a fresh intent
//! pair: she posts a short, a new trader posts a long, and the
//! substitution hands her PMLC long side to that new trader with
//! on-chain PnL settled at the match midpoint. The untouched
//! counterparty (short side) is unaffected.
//!
//! Scenario entry (50) equals the new overlap midpoint (50), so the
//! settled delta is zero — the with-settle path still runs end to end.
//! Rejection: the intents are consumed by the substitution, so a
//! repeat with the same ids fails.

use polyleverage::state::{PMLC_STATUS_LIVE, SIDE_LONG, SIDE_SHORT};
use polyleverage_sim::scenario::{ENTRY_FP, SCENARIO_PRICE_FP, SCENARIO_EXPIRY_SLOT};
use polyleverage_sim::Scenario;
use solana_sdk::signature::Signer;

#[test]
fn substitute_with_settle_hands_off_position() {
    let mut s = Scenario::new();
    let pmlc_pda = s.open_pmlc();

    // Carol will take over the long side.
    let carol = s.new_funded_trader();

    // The original long owner posts a short to exit; Carol posts a long.
    let short_id = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &s.long,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_SHORT,
        SCENARIO_PRICE_FP,
        1,
        SCENARIO_EXPIRY_SLOT,
    )
    .expect("exiting short intent");

    let long_id = s.h.book_next_intent_id(&s.book);
    s.h.post_intent(
        &carol,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_LONG,
        SCENARIO_PRICE_FP,
        1,
        SCENARIO_EXPIRY_SLOT,
    )
    .expect("carol's long intent");

    s.h.match_substitute_with_settle(
        &s.instrument,
        &s.book,
        &pmlc_pda,
        &s.mint,
        &s.long,
        &carol.pubkey(),
        long_id,
        short_id,
    )
    .expect("substitute with settle");

    let pmlc = s.h.load_pmlc(&pmlc_pda);
    assert_eq!(
        pmlc.long_owner,
        carol.pubkey(),
        "long side handed to the substituting trader"
    );
    assert_eq!(
        pmlc.short_owner,
        s.short.pubkey(),
        "untouched counterparty unaffected"
    );
    assert_eq!(pmlc.status, PMLC_STATUS_LIVE, "PMLC still live");
    assert_eq!(
        pmlc.entry_price_fp, ENTRY_FP,
        "entry price preserved across substitution"
    );

    // The intents were consumed — repeating the substitution must fail.
    let repeat = s.h.match_substitute_with_settle(
        &s.instrument,
        &s.book,
        &pmlc_pda,
        &s.mint,
        &s.long,
        &carol.pubkey(),
        long_id,
        short_id,
    );
    assert!(
        repeat.is_err(),
        "substituting against already-consumed intents must fail"
    );
}
