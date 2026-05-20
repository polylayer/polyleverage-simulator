# polyleverage compute-unit benchmarks

Per-instruction compute units, measured in-process with litesvm
(`tests/benchmarks.rs`). Regenerate with:

```sh
cargo test --test benchmarks -- --nocapture
```

The benchmark asserts every instruction stays under the 200,000 CU
per-instruction ceiling, so a CU regression fails the test suite.

## Results

Representative single-contract inputs, default 64-node intent book,
zero-fee schedule.

| Instruction                | Compute units | Notes |
|----------------------------|--------------:|-------|
| ClosePmlc                  |         1,356 | Lamport sweep + data zero. |
| PostIntent (long)          |         5,071 | First post on an empty book. |
| PostIntent (short)         |         6,774 | Second post; seat/tree insert. |
| Novate                     |         9,633 | Two margin updates + owner swap. |
| Resolve (+attestation)     |        11,360 | Includes the Ed25519 precompile verify. |
| Withdraw                   |        15,408 | Includes the SPL token transfer CPI. |
| Deposit                    |        15,553 | Includes the SPL token transfer CPI. |
| Liquidate (+attestation)   |        16,302 | Includes the Ed25519 precompile verify. |
| MatchPair                  |        37,623 | Hot path — see below. |

## Hot path

**MatchPair** is the heaviest instruction at ~37.6k CU. It does the
most work in one transaction: resolves both intents, runs the matching
math, lazily creates the fee treasury and both traders' volume
accounts on first use, and allocates the PMLC PDA. Subsequent matches
on the same market are cheaper (treasury + volume accounts already
exist).

`PostIntent` with inline matching (`try_match = 1`) is not measured
separately here — it is approximately `PostIntent` + `MatchPair` in a
single transaction (~43k CU), still far under budget.

All instructions sit comfortably within the 200k per-instruction
limit; the protocol has substantial CU headroom.

## Scaling with book depth

`MatchPair`, `CancelIntent`, and `PostIntent` resolve and prune
intents by scanning the book's node pool, so their cost grows with the
book's provisioned node capacity (not its live fill — the scan visits
every slot). `compute_unit_scaling` measures this directly: it grows a
book to a target capacity and meters `PostIntent` and `MatchPair`.

| Book capacity | PostIntent CU | MatchPair CU |
|--------------:|--------------:|-------------:|
|            16 |         5,679 |       36,336 |
|            64 |         6,308 |       37,020 |
|           256 |        10,304 |       39,324 |
|         1,024 |        21,788 |       53,040 |
|         4,096 |        58,724 |       88,404 |
|         8,192 |       113,472 |      139,056 |

Both grow linearly at roughly 13 CU per node. `MatchPair` fits
`~36,000 CU + ~12.6 CU x capacity`.

The practical ceilings that follow from the linear fit:

- A book of up to **~13,000 node slots** keeps `MatchPair` within the
  200,000 CU default per-instruction limit.
- Raising the transaction's compute budget to the 1,400,000 CU maximum
  (a one-instruction `ComputeBudget` request) extends that to
  **~105,000 slots**, which is also near the 10 MiB account-size cap
  (~109,000 nodes at 96 bytes each). The two ceilings roughly coincide.

For context, the protocol shards by instrument: every
(asset, leverage, bucket) is its own book account. A single instrument
would need on the order of 13,000 simultaneously resting intents
before matching even needs a raised compute budget. That is far above
realistic depth for a per-bucket market.

One operational note: a book account cannot be created deep. Solana
caps account growth at ~10 KiB per transaction, so a book is created
small and grown with repeated `ExpandIntentBook` calls (~106 nodes
each). Provisioning a deep book is a one-time sequence of expand
transactions.

If a single instrument ever needs a book beyond these bounds, the
`O(n)` scans can be removed by having callers pass the intent's node
index (validated against its id) instead of an id the program must
search for, which flattens `MatchPair` to its ~36k base regardless of
depth.
