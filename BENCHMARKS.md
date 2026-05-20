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
