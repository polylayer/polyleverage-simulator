//! Multi-asset verification.
//!
//! Extending polyleverage to equities / commodities / crypto majors
//! priced off Pyth uses a non-Polymarket `source` byte, normalized
//! prices, and the existing leverage / collateral buckets. These tests
//! exercise the program at the new bucket extremes and confirm the
//! bounded-win invariant holds at 1000x leverage with no arithmetic
//! overflow.

use polyleverage::state::{CLOSE_REASON_LIQUIDATED, PMLC_STATUS_LIQUIDATED, PMLC_STATUS_LIVE};
use polyleverage_sim::{normalize_price, InstrumentParams, Scenario};
use solana_sdk::signature::Signer;

#[test]
fn thousand_x_leverage_full_lifecycle_holds_bounded_win() {
    // 1000x leverage, $1000 margin bucket, Pyth source.
    let params = InstrumentParams {
        source: 3, // source::PYTH
        leverage_bps: 10_000_000,         // 1000x
        collateral_bucket: 1_000_000_000, // $1000 at 6 decimals
        ..InstrumentParams::default()
    };
    let mut s = Scenario::with_params(params);
    let bucket = s.params.collateral_bucket;

    let pmlc_pda = s.open_pmlc();
    assert_eq!(
        s.h.load_pmlc(&pmlc_pda).collateral_per_side,
        bucket,
        "each side locks one $1000 bucket"
    );

    // Liquidate at a breach mark of 0 — the long is wiped out.
    let keeper = s.new_funded_trader();
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
    .expect("liquidate at 1000x must not overflow");

    let pmlc = s.h.load_pmlc(&pmlc_pda);
    assert_eq!(pmlc.status, PMLC_STATUS_LIQUIDATED);
    assert_eq!(pmlc.close_reason, CLOSE_REASON_LIQUIDATED);

    // Bounded-win invariant at 1000x: winner gets 2c - bounty, loser 0,
    // keeper gets the bounty; the three credits sum to exactly 2c.
    let bounty = bucket * s.params.liquidation_bounty_bps as u64 / 10_000;
    let long_m = s.h.load_margin(&s.long.pubkey(), &s.mint);
    let short_m = s.h.load_margin(&s.short.pubkey(), &s.mint);
    let keeper_m = s.h.load_margin(&keeper.pubkey(), &s.mint);

    assert_eq!(
        long_m.free_collateral,
        s.trader_deposit - bucket,
        "loser recovers nothing of the locked collateral"
    );
    assert_eq!(
        short_m.free_collateral,
        (s.trader_deposit - bucket) + (2 * bucket - bounty),
        "winner collects 2c minus the keeper bounty"
    );
    assert_eq!(
        keeper_m.free_collateral,
        s.trader_deposit + bounty,
        "keeper collects the bounty"
    );
    assert_eq!(long_m.locked_collateral, 0);
    assert_eq!(short_m.locked_collateral, 0);

    // The three settlement credits sum to exactly 2c — no value created
    // or destroyed at extreme leverage.
    let credited = (long_m.free_collateral - (s.trader_deposit - bucket))
        + (short_m.free_collateral - (s.trader_deposit - bucket))
        + (keeper_m.free_collateral - s.trader_deposit);
    assert_eq!(credited, 2 * bucket, "bilateral equity sum must equal 2c");
}

#[test]
fn small_bucket_low_notional_instrument_matches() {
    // $100 margin bucket at 100x — the other corner of the bucket matrix.
    let params = InstrumentParams {
        source: 3, // source::PYTH
        leverage_bps: 1_000_000,        // 100x
        collateral_bucket: 100_000_000, // $100 at 6 decimals
        ..InstrumentParams::default()
    };
    let mut s = Scenario::with_params(params);

    let pmlc_pda = s.open_pmlc();
    let pmlc = s.h.load_pmlc(&pmlc_pda);
    assert_eq!(pmlc.status, PMLC_STATUS_LIVE);
    assert_eq!(pmlc.collateral_per_side, 100_000_000);
    assert_eq!(pmlc.long_owner, s.long.pubkey());
    assert_eq!(pmlc.short_owner, s.short.pubkey());
}

#[test]
fn normalized_pyth_price_settles_full_lifecycle() {
    // A representative BTC price as Pyth publishes it: $66,849.09218226,
    // raw 6_684_909_218_226 at expo -8. Normalized into the program's
    // (0,1) fixed-point against a $1,000,000 reference ceiling. This is
    // the same integer formula pyth_feed.py --reference applies.
    let entry_fp = normalize_price(6_684_909_218_226, -8, 1_000_000);
    assert_eq!(entry_fp, 66_849_092_182_260_000);

    // A 20x BTC instrument with a $1000 margin bucket, Pyth source.
    let params = InstrumentParams {
        source: 3, // source::PYTH
        leverage_bps: 200_000,
        collateral_bucket: 1_000_000_000,
        ..InstrumentParams::default()
    };
    let mut s = Scenario::with_params(params);

    // Open a position at exactly the normalized Pyth price.
    let pmlc_pda = s.open_pmlc_at(entry_fp);
    assert_eq!(
        s.h.load_pmlc(&pmlc_pda).entry_price_fp,
        entry_fp,
        "PMLC entry settles at the normalized Pyth price"
    );

    // Liquidate at a normalized breach 50% below entry — at 20x this
    // wipes the long out. The bounded-win invariant must hold at a
    // real-magnitude normalized price just as at the abstract ones.
    let keeper = s.new_funded_trader();
    s.h.liquidate(
        &s.attestor,
        &s.instrument,
        &pmlc_pda,
        &s.mint,
        s.params.market_id,
        &keeper,
        &s.long.pubkey(),
        &s.short.pubkey(),
        entry_fp / 2,
        1,
    )
    .expect("liquidate at a normalized breach price");

    let pmlc = s.h.load_pmlc(&pmlc_pda);
    assert_eq!(pmlc.status, PMLC_STATUS_LIQUIDATED);
    assert_eq!(pmlc.close_reason, CLOSE_REASON_LIQUIDATED);

    let bucket = s.params.collateral_bucket;
    let bounty = bucket * s.params.liquidation_bounty_bps as u64 / 10_000;
    let short_m = s.h.load_margin(&s.short.pubkey(), &s.mint);
    assert_eq!(
        short_m.free_collateral,
        (s.trader_deposit - bucket) + (2 * bucket - bounty),
        "winner collects 2c - bounty at a normalized price"
    );
}
