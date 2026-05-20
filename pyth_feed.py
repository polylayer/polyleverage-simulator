#!/usr/bin/env python3
"""
Pyth price feeder for the polyleverage local simulation harness.

The polyleverage program settles synthetic positions against a price
the TEE/CRE attestor signs. To run the program locally we need a price
source; this script is it. It pulls from Pyth (the same oracle the
production attestor uses) so local runs exercise realistic numbers,
and it has a deterministic --mock mode for reproducible tests.

Two Pyth surfaces are used:
  - Hermes  (hermes.pyth.network)      — latest prices, feed discovery
  - Benchmarks (benchmarks.pyth.network) — historical price at a timestamp

Output is a single JSON object the (Rust) attestor helper consumes:
  { "symbol", "feed_id", "price", "expo", "price_decimal",
    "publish_time", "source" }

`price` is the raw integer; the real value is price * 10^expo. The
expo is feed-specific (crypto majors are -8, metals differ — XAU is
-3) so always read it off each response rather than assuming.
`price_decimal` is the convenience float — do NOT use it for on-chain
math, only for display.

Usage:
  pyth_feed.py latest SOL/USD
  pyth_feed.py latest "Crypto.BTC/USD"
  pyth_feed.py historical XAU/USD 1716000000
  pyth_feed.py latest SOL/USD --mock     # deterministic, no network
  pyth_feed.py feeds crypto              # list discoverable feeds

Asset coverage the polyleverage expansion targets: crypto majors
(BTC/ETH/SOL), metals (XAU=gold, XAG=silver), and equities (xStocks).
Pyth carries all three asset classes; this script resolves any of them
by symbol.
"""

import argparse
import json
import sys
import urllib.parse
import urllib.request

HERMES = "https://hermes.pyth.network"
BENCHMARKS = "https://benchmarks.pyth.network"

# Deterministic mock prices (raw integer, expo -8) for offline tests.
# Values are illustrative, not live — only used under --mock.
MOCK_PRICES = {
    "SOL/USD": 8_400_000_000,      # $84.00
    "BTC/USD": 6_700_000_000_000,  # $67,000.00
    "ETH/USD": 350_000_000_000,    # $3,500.00
    "XAU/USD": 240_000_000_000,    # $2,400.00  (gold, troy oz)
    "XAG/USD": 3_000_000_000,      # $30.00     (silver, troy oz)
}
MOCK_EXPO = -8


# Hermes sits behind a CDN that 403s the default urllib user-agent.
_UA = "polyleverage-sim/1.0 (+pyth_feed.py)"


def _get(url: str) -> dict:
    req = urllib.request.Request(
        url, headers={"Accept": "application/json", "User-Agent": _UA}
    )
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read().decode())


def resolve_feed_id(symbol: str) -> str:
    """Resolve a human symbol (e.g. 'SOL/USD' or 'Crypto.BTC/USD') to a
    Pyth feed id by querying Hermes' feed directory. Exact case-folded
    match on the symbol's tail so 'SOL/USD' matches 'Crypto.SOL/USD'."""
    q = urllib.parse.quote(symbol.split(".")[-1])
    feeds = _get(f"{HERMES}/v2/price_feeds?query={q}")
    want = symbol.split(".")[-1].upper()
    for f in feeds:
        attr = f.get("attributes", {})
        sym = attr.get("symbol", "")
        if sym.split(".")[-1].upper() == want:
            return f["id"]
    # Fall back to the first result if no exact tail match.
    if feeds:
        return feeds[0]["id"]
    raise SystemExit(f"no Pyth feed found for symbol {symbol!r}")


def latest_price(feed_id: str) -> dict:
    url = f"{HERMES}/v2/updates/price/latest?ids[]={feed_id}"
    body = _get(url)
    parsed = body.get("parsed") or []
    for p in parsed:
        if p.get("id", "").lower() == feed_id.lower():
            return p["price"]
    raise SystemExit(f"feed {feed_id} missing from Hermes response")


def historical_price(feed_id: str, timestamp: int) -> dict:
    """Price at a past unix timestamp, via Hermes' timestamped update.
    Used for contract settlement at a fixed time T."""
    url = f"{HERMES}/v2/updates/price/{int(timestamp)}?ids[]={feed_id}"
    body = _get(url)
    parsed = body.get("parsed") or []
    for p in parsed:
        if p.get("id", "").lower() == feed_id.lower():
            return p["price"]
    raise SystemExit(f"feed {feed_id} missing from Hermes timestamp response")


# The polyleverage program requires prices in (0, 1e18) fixed-point.
PRICE_ONE = 10**18


def emit(symbol: str, feed_id: str, price: dict, source: str, reference: int = None) -> None:
    raw = int(price["price"])
    expo = int(price["expo"])
    out = {
        "symbol": symbol,
        "feed_id": feed_id,
        "price": raw,
        "expo": expo,
        "price_decimal": raw * (10.0 ** expo),
        "publish_time": int(price["publish_time"]),
        "source": source,
    }
    if reference is not None:
        # Normalize the dollar price into the program's (0,1) fixed-point
        # against a per-instrument reference ceiling:
        # price_fp = raw * 10^(18+expo) / reference.
        # Integer math throughout; the PnL math is ratio-based so the
        # normalization is economically transparent.
        shift = 18 + expo
        if shift < 0:
            raise SystemExit(f"expo {expo} too small to normalize")
        price_fp = raw * (10**shift) // reference
        if not (0 < price_fp < PRICE_ONE):
            raise SystemExit(
                f"normalized price_fp {price_fp} is outside (0, 1e18); "
                f"raise --reference above the asset price"
            )
        out["reference_usd"] = reference
        out["price_fp"] = price_fp
    print(json.dumps(out, indent=2))


def main() -> None:
    ap = argparse.ArgumentParser(description="Pyth price feeder for the polyleverage sim harness")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p_latest = sub.add_parser("latest", help="latest price for a symbol")
    p_latest.add_argument("symbol")
    p_latest.add_argument("--mock", action="store_true", help="deterministic offline price")
    p_latest.add_argument(
        "--reference",
        type=int,
        help="reference ceiling (whole USD); emits a normalized price_fp",
    )

    p_hist = sub.add_parser("historical", help="price at a past unix timestamp")
    p_hist.add_argument("symbol")
    p_hist.add_argument("timestamp", type=int)
    p_hist.add_argument(
        "--reference",
        type=int,
        help="reference ceiling (whole USD); emits a normalized price_fp",
    )

    p_feeds = sub.add_parser("feeds", help="list discoverable Pyth feeds matching a query")
    p_feeds.add_argument("query")

    args = ap.parse_args()

    if args.cmd == "latest":
        sym = args.symbol.split(".")[-1].upper()
        if args.mock:
            if sym not in MOCK_PRICES:
                raise SystemExit(f"--mock has no entry for {sym}; add it to MOCK_PRICES")
            emit(
                sym,
                "0" * 64,
                {"price": MOCK_PRICES[sym], "expo": MOCK_EXPO, "publish_time": 0},
                "mock",
                args.reference,
            )
        else:
            fid = resolve_feed_id(args.symbol)
            emit(sym, fid, latest_price(fid), "hermes", args.reference)

    elif args.cmd == "historical":
        sym = args.symbol.split(".")[-1].upper()
        fid = resolve_feed_id(args.symbol)
        emit(
            sym,
            fid,
            historical_price(fid, args.timestamp),
            "hermes-historical",
            args.reference,
        )

    elif args.cmd == "feeds":
        q = urllib.parse.quote(args.query)
        feeds = _get(f"{HERMES}/v2/price_feeds?query={q}")
        for f in feeds[:50]:
            attr = f.get("attributes", {})
            print(f"{f['id']}  {attr.get('symbol', '?'):28} {attr.get('asset_type', '?')}")


if __name__ == "__main__":
    sys.exit(main())
