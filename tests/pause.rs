//! Emergency pause rehearsal.
//!
//! With the global pause set, mutating instructions (here: PostIntent)
//! must fail. Clearing the pause restores normal operation.

use polyleverage::state::SIDE_LONG;
use polyleverage_sim::scenario::{RANGE_MAX_FP, RANGE_MIN_FP, SCENARIO_EXPIRY_SLOT};
use polyleverage_sim::Scenario;

#[test]
fn emergency_pause_blocks_then_resumes() {
    let mut s = Scenario::new();

    s.h.set_global_pause(true);
    let blocked = s.h.post_intent(
        &s.long,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_LONG,
        RANGE_MIN_FP,
        RANGE_MAX_FP,
        1,
        SCENARIO_EXPIRY_SLOT,
    );
    assert!(
        blocked.is_err(),
        "posting must fail while the program is globally paused"
    );

    s.h.set_global_pause(false);
    let resumed = s.h.post_intent(
        &s.long,
        &s.instrument,
        &s.book,
        &s.mint,
        SIDE_LONG,
        RANGE_MIN_FP,
        RANGE_MAX_FP,
        1,
        SCENARIO_EXPIRY_SLOT,
    );
    assert!(
        resumed.is_ok(),
        "posting resumes once the pause is cleared"
    );
}
