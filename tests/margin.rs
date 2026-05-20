//! SPL deposit / withdraw round trip.
//!
//! Exercises the collateral path end-to-end: a user deposits SPL tokens
//! into the protocol vault (a real token-program CPI), the margin
//! ledger is credited, and a later withdraw moves tokens back out.
//! Also checks the rejection: a withdraw exceeding the free margin
//! balance must fail and leave state untouched.

use polyleverage_sim::{Attestor, Harness};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

#[test]
fn spl_deposit_withdraw_round_trip() {
    let mut h = Harness::new();
    h.init_program_config(Attestor::new().signer_bytes(), 300);

    // 6-decimal USDC-like collateral mint + the protocol vault ATA.
    let mint = h.create_mint(6);
    let vault = h.init_vault_ata(&mint);

    // User funded with 1,000 tokens in an account they control.
    let user = h.create_user();
    let user_ata = Pubkey::new_unique();
    h.create_token_account(&user_ata, &mint, &user.pubkey(), 1_000_000_000);

    h.create_margin_account(&user, &mint);

    // Deposit 600 → vault holds it, user account drained by that much.
    h.deposit(&user, &mint, &user_ata, 600_000_000).expect("deposit");
    assert_eq!(h.token_balance(&user_ata), 400_000_000);
    assert_eq!(h.token_balance(&vault), 600_000_000);

    // Withdraw 250 → flows back to the user.
    h.withdraw(&user, &mint, &user_ata, 250_000_000)
        .expect("withdraw");
    assert_eq!(h.token_balance(&user_ata), 650_000_000);
    assert_eq!(h.token_balance(&vault), 350_000_000);

    // Withdrawing more than the 350 free balance must be rejected.
    assert!(
        h.withdraw(&user, &mint, &user_ata, 400_000_000).is_err(),
        "over-withdraw of the free margin balance must fail"
    );
    // The rejected transaction must not have moved any tokens.
    assert_eq!(h.token_balance(&user_ata), 650_000_000);
    assert_eq!(h.token_balance(&vault), 350_000_000);
}
