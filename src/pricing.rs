//! Off-chain price normalization.
//!
//! Pyth publishes a dollar price as a `(raw, expo)` pair; the real
//! value is `raw * 10^expo`. The polyleverage program requires every
//! price in its `(0, 1e18)` fixed-point. Normalization maps a dollar
//! price into that range against a per-instrument reference ceiling.
//! Because the program's PnL math is ratio-based (PnL = notional ×
//! return), the choice of reference scales out exactly, so the
//! normalization is economically transparent.
//!
//! This is the Rust counterpart of `pyth_feed.py`'s `--reference`
//! output; both implement the identical integer formula.

/// The program's price fixed-point unit (1.0 == 1e18).
pub const PRICE_ONE: u128 = 1_000_000_000_000_000_000;

/// Normalize a Pyth dollar price into the program's `(0, 1e18)`
/// fixed-point against `reference_usd`, a whole-dollar ceiling:
///
/// ```text
/// price_fp = raw * 10^(18 + expo) / reference_usd
/// ```
///
/// Panics if the result is not strictly inside `(0, 1e18)` — that
/// means the reference ceiling is at or below the asset price and
/// must be raised. (For the multi-asset launch set the references are
/// chosen with large headroom, so this never fires in practice.)
pub fn normalize_price(raw: u64, expo: i32, reference_usd: u64) -> u64 {
    let shift = 18 + expo;
    assert!(shift >= 0, "expo {expo} too small to normalize");
    let scaled = (raw as u128)
        .checked_mul(10u128.pow(shift as u32))
        .expect("normalize_price: overflow scaling the raw price");
    let price_fp = scaled / (reference_usd as u128);
    assert!(
        price_fp > 0 && price_fp < PRICE_ONE,
        "normalized price_fp {price_fp} is outside (0, 1e18); \
         raise the reference ceiling above the asset price"
    );
    price_fp as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_a_pyth_btc_price() {
        // BTC at $66,849.09218226 as Pyth publishes it, expo -8,
        // against a $1,000,000 reference. Matches pyth_feed.py:
        //   raw * 10^(18-8) / 1_000_000.
        let fp = normalize_price(6_684_909_218_226, -8, 1_000_000);
        assert_eq!(fp, 66_849_092_182_260_000);
        assert!(fp > 0 && (fp as u128) < PRICE_ONE);
    }

    #[test]
    fn reference_scales_out_of_a_price_ratio() {
        // Two references give different fixed-points, but the ratio
        // between two prices is preserved — which is what makes PnL
        // (a ratio) normalization-invariant.
        let a = normalize_price(2_000_000_000, -8, 1_000); // $20 / $1k
        let b = normalize_price(4_000_000_000, -8, 1_000); // $40 / $1k
        assert_eq!(b, 2 * a);
        let a2 = normalize_price(2_000_000_000, -8, 100_000); // $20 / $100k
        let b2 = normalize_price(4_000_000_000, -8, 100_000); // $40 / $100k
        assert_eq!(b2, 2 * a2);
    }

    #[test]
    #[should_panic(expected = "outside (0, 1e18)")]
    fn rejects_a_reference_below_the_price() {
        // $84 against a $50 reference would normalize above 1.0.
        normalize_price(8_400_000_000, -8, 50);
    }
}
