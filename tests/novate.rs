//! Novate: transfer one side of a live PMLC to a new owner.
//!
//! The long side of a matched PMLC is novated from its original owner
//! to a fresh trader. Entry price and the counterparty are preserved.
//! Rejection: a caller who does not own the named side cannot novate it.

use polyleverage::state::{PMLC_STATUS_LIVE, SIDE_LONG};
use polyleverage_sim::Scenario;
use solana_sdk::signature::Signer;

#[test]
fn novate_transfers_pmlc_side_to_new_owner() {
    let mut s = Scenario::new();
    let pmlc_pda = s.open_pmlc();
    let entry_before = s.h.load_pmlc(&pmlc_pda).entry_price_fp;

    let new_owner = s.new_funded_trader();
    s.h.novate(
        &s.instrument,
        &pmlc_pda,
        &s.mint,
        SIDE_LONG,
        &s.long,
        &new_owner,
    )
    .expect("novate long side");

    let pmlc = s.h.load_pmlc(&pmlc_pda);
    assert_eq!(
        pmlc.long_owner,
        new_owner.pubkey(),
        "long side transferred to the new owner"
    );
    assert_eq!(
        pmlc.short_owner,
        s.short.pubkey(),
        "short side (counterparty) untouched"
    );
    assert_eq!(pmlc.status, PMLC_STATUS_LIVE, "PMLC still live");
    assert_eq!(
        pmlc.entry_price_fp, entry_before,
        "entry price preserved across novation"
    );
}

#[test]
fn novate_rejects_caller_who_does_not_own_the_side() {
    let mut s = Scenario::new();
    let pmlc_pda = s.open_pmlc();
    let new_owner = s.new_funded_trader();

    // s.short owns the SHORT side; it must not be able to novate the LONG.
    let res = s.h.novate(
        &s.instrument,
        &pmlc_pda,
        &s.mint,
        SIDE_LONG,
        &s.short,
        &new_owner,
    );
    assert!(
        res.is_err(),
        "novating a side the caller does not own must be rejected"
    );
}
