# polyleverage-simulator

An in-process simulation, testing, and benchmarking harness for the
[`polyleverage`](https://github.com/polylayer/polyleverage) Solana
program. It compiles the program to SBF bytecode, loads that bytecode
into an in-process Solana virtual machine, and drives it end to end
with a simulated CRE/TEE attestor and a Pyth-backed price source.

The program being tested is real: real SBF bytecode, real
transactions, real cross-program invocations, real compute-unit
metering. Only the off-chain operators (the attestor, the oracle) are
stood in for. See the architecture writeup linked below for the
reasoning.

## Why in-process

The harness runs on [litesvm](https://github.com/LiteSVM/litesvm):
the program executes inside the test process, with no validator
daemon and no network. That buys three things a
`solana-test-validator` setup cannot:

- **Speed.** A full end-to-end test runs in milliseconds.
- **Determinism.** No wall clock, no block-production race.
- **Direct clock control.** The `Clock` sysvar is set outright, so
  the 24-hour-class governance timelocks are exercised instantly
  rather than waited out.

`solana-test-validator` is only needed to test real RPC and
continuous block production, which the program logic does not depend
on. litesvm covers the entire suite.

## Layout

The `polyleverage` program is included as a git submodule, so the
harness builds and loads the exact program source it tests.

```
polyleverage-simulator/
├── polyleverage/      # git submodule — the program under test
├── src/
│   ├── driver.rs      # Harness: program load, PDAs, tx submission, ix helpers
│   ├── attestor.rs    # simulated CRE/TEE attestor (Ed25519, attestation framing)
│   ├── scenario.rs    # Scenario: full-market builder + open-a-PMLC helper
│   └── pricing.rs     # off-chain Pyth price normalization
├── tests/             # the end-to-end, adversarial, and benchmark suite
├── pyth_feed.py       # Pyth price feeder (live + historical + mock)
└── BENCHMARKS.md      # committed compute-unit table
```

## Build and run

The harness loads the program's compiled artifact, so clone with
submodules and build the program first.

```sh
git clone --recurse-submodules https://github.com/polylayer/polyleverage-simulator
cd polyleverage-simulator

# build the program under test
( cd polyleverage && cargo build-sbf )

# run the suite
cargo test

# compute-unit benchmark table
cargo test --test benchmarks -- --nocapture
```

Prerequisites: a Rust toolchain, the Solana SBF toolchain
(`solana-cargo-build-sbf`), and Python 3 for `pyth_feed.py` (standard
library only, no pip dependencies).

## What the suite covers

Thirty-six test functions across twelve files, in four layers.

- **End to end** — the full protocol lifecycle: SPL deposit and
  withdrawal, instrument creation, intent posting and matching into a
  position, liquidation, resolution, position close, novation,
  substitution, the governance timelock, and the emergency pause.
- **Adversarial** — every attestation-forgery vector (wrong signer,
  wrong type, wrong position binding, replayed nonce, wrong market,
  missing attestation) and the malformed-intent surface, each
  confirmed to be rejected.
- **Performance** — every instruction metered for compute units, with
  a hard ceiling asserted so a regression fails the suite. Numbers
  in `BENCHMARKS.md`.
- **Multi-asset** — the program driven at the leverage and margin
  bucket extremes (up to 1000x), and a full lifecycle on a normalized
  real Pyth price.

## The simulated attestor

In production the program settles against attestations signed by a
TEE-held Ed25519 key. `src/attestor.rs` stands in for that TEE: it
builds the exact 104-byte attestation layouts and the
`Ed25519SigVerify` precompile instruction, using the program crate's
own layout constants so the harness cannot drift from the on-chain
wire format. The signatures it produces are real Ed25519 signatures,
verified for real by the on-chain program.

## Pyth price feeder

`pyth_feed.py` supplies oracle prices:

```sh
python3 pyth_feed.py latest SOL/USD                   # live, from Pyth Hermes
python3 pyth_feed.py latest BTC/USD --reference 1000000  # normalized price_fp
python3 pyth_feed.py historical XAU/USD 1716000000    # price at a past timestamp
python3 pyth_feed.py latest SOL/USD --mock            # deterministic, offline
python3 pyth_feed.py feeds GOOGLX                     # discover feeds
```

It resolves crypto, metals, and equity (xStocks) feeds by symbol, and
with `--reference` emits a price normalized into the program's
fixed-point.

## Further reading

The design of the protocol and this harness is described in
**The Polyleverage Protocol Architecture**, in the `docs/` directory
of the [`polyleverage`](https://github.com/polylayer/polyleverage)
repository.

## License

Apache-2.0.
