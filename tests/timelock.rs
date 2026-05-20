//! Governance timelock execution + cancellation.
//!
//! Attestation-signer rotation is gated behind a fixed-delay timelock.
//! One test: a proposal executes only after the delay elapses
//! (clock-warped in-process). The other: a cancelled proposal can
//! never execute, even once the delay has passed.

use polyleverage::state::TIMELOCK_DELAY_SECS;
use polyleverage_sim::{Attestor, Harness};

#[test]
fn timelock_executes_only_after_delay() {
    let mut h = Harness::new();
    let original = Attestor::new();
    h.init_program_config(original.signer_bytes(), 300);

    let proposed = Attestor::new();
    let pid = 1u64;
    h.propose_set_attestation_signer(pid, proposed.signer_bytes());

    // Before the delay elapses, execution must be rejected.
    assert!(
        h.execute_set_attestation_signer(pid).is_err(),
        "timelock must not execute before the delay"
    );

    // Warp past the delay, then execute.
    h.warp_unix(TIMELOCK_DELAY_SECS + 1);
    h.execute_set_attestation_signer(pid)
        .expect("execute after the delay");

    assert_eq!(
        h.load_program_config().attestation_signer.to_bytes(),
        proposed.signer_bytes(),
        "attestation signer rotated to the proposed key"
    );
}

#[test]
fn timelock_cancel_blocks_execution() {
    let mut h = Harness::new();
    let original = Attestor::new();
    h.init_program_config(original.signer_bytes(), 300);

    let proposed = Attestor::new();
    let pid = 7u64;
    h.propose_set_attestation_signer(pid, proposed.signer_bytes());
    h.cancel_timelock(pid).expect("cancel");

    // Even after the delay, a cancelled proposal must not execute.
    h.warp_unix(TIMELOCK_DELAY_SECS + 1);
    assert!(
        h.execute_set_attestation_signer(pid).is_err(),
        "a cancelled proposal must never execute"
    );

    assert_eq!(
        h.load_program_config().attestation_signer.to_bytes(),
        original.signer_bytes(),
        "attestation signer unchanged after a cancelled proposal"
    );
}
