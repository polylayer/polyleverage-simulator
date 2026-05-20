//! Smoke test: prove the harness substrate works end-to-end.
//!
//! Loads the SBF program into litesvm, runs a real program instruction
//! (`InitProgramConfig`, which CPIs to the system program to allocate
//! its PDA), and verifies the resulting on-chain state. If this passes,
//! the harness can load + execute the program and the E2E suite in
//! `tests/` can build on it.

use polyleverage::state::PROGRAM_CONFIG_LEN;
use polyleverage_sim::{Attestor, Harness};

#[test]
fn init_program_config_creates_config_pda() {
    let mut h = Harness::new();
    let attestor = Attestor::new();

    let config = h.init_program_config(attestor.signer_bytes(), 300);

    let acct = h
        .account(&config)
        .expect("config PDA should exist after init");
    assert_eq!(
        acct.owner, h.program_id,
        "config PDA must be owned by the polyleverage program"
    );
    assert_eq!(
        acct.data.len(),
        PROGRAM_CONFIG_LEN,
        "config account allocated to ProgramConfig size"
    );
    assert!(acct.lamports > 0, "config PDA must be rent-funded");
}
