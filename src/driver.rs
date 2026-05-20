//! Harness driver: load the program into litesvm and drive it.
//!
//! `Harness` owns an in-process SVM with the polyleverage SBF program
//! loaded. It exposes the primitives every E2E test composes — PDA
//! derivation, transaction submission, instruction framing — plus
//! typed helpers for the program's instructions, added as the test
//! suite grows.

use borsh::BorshSerialize;
use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    clock::Clock,
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    program_option::COption,
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    sysvar::instructions as sysvar_instructions,
    system_program,
    transaction::Transaction,
};
use spl_token::state::{Account as TokenAccount, AccountState, Mint};

use crate::attestor::Attestor;
use polyleverage::instruction::{
    CancelTimelockArgs, CreateInstrumentArgs, DepositArgs, ExecuteTimelockArgs,
    ExpandIntentBookArgs, FeeTier, InitFeeScheduleArgs, InitProgramConfigArgs, InstructionTag,
    LiquidateArgs, MatchPairArgs, NovateArgs, PostIntentArgs, ProposeSetAttestationSignerArgs,
    ResolveArgs, WithdrawArgs,
};
use polyleverage::seeds::{
    SEED_BOOK, SEED_CONFIG, SEED_FEE_SCHEDULE, SEED_INSTRUMENT, SEED_MARGIN, SEED_MARKET_NONCE,
    SEED_TIMELOCK, SEED_TREASURY, SEED_USER_VOLUME, SEED_VAULT,
};
use polyleverage::state::{
    IntentBookHeader, MarginAccount, Pmlc, ProgramConfig, INTENT_BOOK_HEADER_LEN,
};

/// One lamport-funded SOL, for airdrops.
pub const SOL: u64 = 1_000_000_000;

/// Result of a submitted transaction.
pub type TxResult = litesvm::types::TransactionResult;

/// In-process polyleverage runtime.
pub struct Harness {
    pub svm: LiteSVM,
    pub program_id: Pubkey,
    /// Default fee payer; also the admin authority after `init_program_config`.
    pub payer: Keypair,
}

impl Harness {
    /// Build a fresh runtime with the prebuilt SBF artifact loaded.
    ///
    /// Panics if `polyleverage.so` is missing — run `cargo build-sbf`
    /// in `../polyleverage` first (see the harness README).
    pub fn new() -> Self {
        let so_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/polyleverage/target/deploy/polyleverage.so"
        );
        let program_bytes = std::fs::read(so_path).unwrap_or_else(|e| {
            panic!(
                "cannot read {so_path}: {e}\n\
                 build it first: (cd polyleverage && cargo build-sbf)"
            )
        });

        let program_id = polyleverage::ID;
        let mut svm = LiteSVM::new();
        svm.add_program(program_id, &program_bytes);

        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000 * SOL)
            .expect("payer airdrop");

        Self {
            svm,
            program_id,
            payer,
        }
    }

    /// Fund an arbitrary account with lamports.
    pub fn airdrop(&mut self, who: &Pubkey, lamports: u64) {
        self.svm.airdrop(who, lamports).expect("airdrop");
    }

    /// Derive a program PDA from the given seeds.
    pub fn pda(&self, seeds: &[&[u8]]) -> (Pubkey, u8) {
        Pubkey::find_program_address(seeds, &self.program_id)
    }

    /// The singleton `ProgramConfig` PDA.
    pub fn config_pda(&self) -> Pubkey {
        self.pda(&[SEED_CONFIG]).0
    }

    /// Fetch an account's current state.
    pub fn account(&self, key: &Pubkey) -> Option<Account> {
        self.svm.get_account(key)
    }

    /// Frame a program instruction: one tag byte followed by the
    /// Borsh-encoded args (the program's wire format).
    pub fn ix<A: BorshSerialize>(
        &self,
        tag: InstructionTag,
        args: &A,
        accounts: Vec<AccountMeta>,
    ) -> Instruction {
        let mut data = vec![tag as u8];
        data.extend(borsh::to_vec(args).expect("borsh encode"));
        Instruction {
            program_id: self.program_id,
            accounts,
            data,
        }
    }

    /// Submit a transaction signed by the payer plus any extra signers.
    ///
    /// The blockhash is expired first so every transaction is unique —
    /// otherwise two structurally identical transactions (e.g. the same
    /// post attempted before and after a pause) collide on signature
    /// and the second is rejected as a replay.
    pub fn send(&mut self, ixs: &[Instruction], extra_signers: &[&Keypair]) -> TxResult {
        self.svm.expire_blockhash();
        let blockhash = self.svm.latest_blockhash();
        let mut signers: Vec<&Keypair> = vec![&self.payer];
        signers.extend_from_slice(extra_signers);
        let tx = Transaction::new_signed_with_payer(
            ixs,
            Some(&self.payer.pubkey()),
            &signers,
            blockhash,
        );
        self.svm.send_transaction(tx)
    }

    /// `InitProgramConfig` — creates the singleton config PDA with the
    /// payer as admin authority. Returns the config PDA.
    pub fn init_program_config(
        &mut self,
        attestation_signer: [u8; 32],
        default_max_staleness_secs: u64,
    ) -> Pubkey {
        let config = self.config_pda();
        let fee_treasury_authority = Pubkey::new_unique();
        let args = InitProgramConfigArgs {
            attestation_signer,
            default_max_staleness_secs,
        };
        let ix = self.ix(
            InstructionTag::InitProgramConfig,
            &args,
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(config, false),
                AccountMeta::new_readonly(fee_treasury_authority, false),
                AccountMeta::new_readonly(system_program::ID, false),
            ],
        );
        self.send(&[ix], &[]).expect("init_program_config");
        config
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

// --- SPL token fixtures -----------------------------------------------------
//
// The program never creates mints or token accounts itself — it expects
// them to exist. The harness sets them directly in the SVM (packed SPL
// state) rather than running the token program's init instructions; the
// program's own deposit/withdraw CPIs still exercise the real token
// program loaded in litesvm.

impl Harness {
    /// Create an initialized SPL mint; the payer is its mint authority.
    pub fn create_mint(&mut self, decimals: u8) -> Pubkey {
        let mint = Pubkey::new_unique();
        let mut data = vec![0u8; Mint::LEN];
        Mint {
            mint_authority: COption::Some(self.payer.pubkey()),
            supply: 0,
            decimals,
            is_initialized: true,
            freeze_authority: COption::None,
        }
        .pack_into_slice(&mut data);
        self.set_spl_account(&mint, data);
        mint
    }

    /// Create an SPL token account at `address`, token-authority `authority`,
    /// holding `amount` of `mint`.
    pub fn create_token_account(
        &mut self,
        address: &Pubkey,
        mint: &Pubkey,
        authority: &Pubkey,
        amount: u64,
    ) {
        let mut data = vec![0u8; TokenAccount::LEN];
        TokenAccount {
            mint: *mint,
            owner: *authority,
            amount,
            delegate: COption::None,
            state: AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        }
        .pack_into_slice(&mut data);
        self.set_spl_account(address, data);
    }

    fn set_spl_account(&mut self, address: &Pubkey, data: Vec<u8>) {
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                *address,
                Account {
                    lamports,
                    data,
                    owner: spl_token::ID,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .expect("set spl account");
    }

    /// Read an SPL token account's balance.
    pub fn token_balance(&self, address: &Pubkey) -> u64 {
        let acct = self.account(address).expect("token account exists");
        TokenAccount::unpack(&acct.data)
            .expect("token account unpacks")
            .amount
    }

    /// The `[SEED_VAULT, mint]` vault-authority PDA.
    pub fn vault_authority(&self, mint: &Pubkey) -> Pubkey {
        self.pda(&[SEED_VAULT, mint.as_ref()]).0
    }

    /// The canonical protocol collateral vault ATA for `mint`.
    pub fn vault_ata(&self, mint: &Pubkey) -> Pubkey {
        spl_associated_token_account::get_associated_token_address(
            &self.vault_authority(mint),
            mint,
        )
    }

    /// Create the (empty) protocol vault ATA at its canonical address.
    pub fn init_vault_ata(&mut self, mint: &Pubkey) -> Pubkey {
        let authority = self.vault_authority(mint);
        let ata = self.vault_ata(mint);
        self.create_token_account(&ata, mint, &authority, 0);
        ata
    }

    /// The `[SEED_MARGIN, owner, mint]` margin-account PDA.
    pub fn margin_pda(&self, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        self.pda(&[SEED_MARGIN, owner.as_ref(), mint.as_ref()]).0
    }
}

// --- Users ------------------------------------------------------------------

impl Harness {
    /// Create a fresh SOL-funded keypair to act as a trader.
    pub fn create_user(&mut self) -> Keypair {
        let kp = Keypair::new();
        self.svm
            .airdrop(&kp.pubkey(), 100 * SOL)
            .expect("user airdrop");
        kp
    }
}

// --- Margin instruction helpers ---------------------------------------------

impl Harness {
    /// Frame a program instruction with no payload (tag byte only).
    pub fn ix_bare(&self, tag: InstructionTag, accounts: Vec<AccountMeta>) -> Instruction {
        Instruction {
            program_id: self.program_id,
            accounts,
            data: vec![tag as u8],
        }
    }

    /// `CreateMarginAccount` for `owner` + `mint`. Returns the margin PDA.
    pub fn create_margin_account(&mut self, owner: &Keypair, mint: &Pubkey) -> Pubkey {
        let margin = self.margin_pda(&owner.pubkey(), mint);
        let ix = self.ix_bare(
            InstructionTag::CreateMarginAccount,
            vec![
                AccountMeta::new(owner.pubkey(), true),
                AccountMeta::new(margin, false),
                AccountMeta::new_readonly(*mint, false),
                AccountMeta::new_readonly(system_program::ID, false),
            ],
        );
        self.send(&[ix], &[owner]).expect("create_margin_account");
        margin
    }

    /// `Deposit` — move `amount` from `owner`'s `user_ata` into the protocol
    /// vault and credit `owner`'s margin account.
    pub fn deposit(
        &mut self,
        owner: &Keypair,
        mint: &Pubkey,
        user_ata: &Pubkey,
        amount: u64,
    ) -> TxResult {
        let ix = self.ix(
            InstructionTag::Deposit,
            &DepositArgs { amount },
            vec![
                AccountMeta::new_readonly(owner.pubkey(), true),
                AccountMeta::new(self.margin_pda(&owner.pubkey(), mint), false),
                AccountMeta::new_readonly(*mint, false),
                AccountMeta::new(*user_ata, false),
                AccountMeta::new(self.vault_ata(mint), false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
        );
        self.send(&[ix], &[owner])
    }

    /// `Withdraw` — move `amount` from the protocol vault back to `owner`'s
    /// `user_ata`.
    pub fn withdraw(
        &mut self,
        owner: &Keypair,
        mint: &Pubkey,
        user_ata: &Pubkey,
        amount: u64,
    ) -> TxResult {
        let ix = self.ix(
            InstructionTag::Withdraw,
            &WithdrawArgs { amount },
            vec![
                AccountMeta::new_readonly(owner.pubkey(), true),
                AccountMeta::new(self.margin_pda(&owner.pubkey(), mint), false),
                AccountMeta::new_readonly(*mint, false),
                AccountMeta::new(*user_ata, false),
                AccountMeta::new(self.vault_ata(mint), false),
                AccountMeta::new_readonly(self.vault_authority(mint), false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
        );
        self.send(&[ix], &[owner])
    }
}

// --- Trading: instrument, fee schedule, intents, matching -------------------

/// Parameters for `Harness::create_instrument`. Mirrors `CreateInstrumentArgs`
/// minus the `source_token_id_*` fields (opaque to the program; left zeroed).
pub struct InstrumentParams {
    pub source: u8,
    pub market_id: [u8; 32],
    pub leverage_bps: u32,
    pub collateral_bucket: u64,
    pub twap_window_slots: u64,
    pub tick_fp: u64,
    pub liquidation_bps: u32,
    pub liquidation_bounty_bps: u16,
    pub max_staleness_secs: u64,
    pub initial_book_capacity: u32,
}

impl Default for InstrumentParams {
    fn default() -> Self {
        Self {
            source: 0,
            market_id: [7u8; 32],
            leverage_bps: 20_000,
            collateral_bucket: 1_000_000,
            twap_window_slots: 100,
            tick_fp: 1,
            liquidation_bps: 5_000,
            liquidation_bounty_bps: 100,
            max_staleness_secs: 300,
            initial_book_capacity: 64,
        }
    }
}

impl Harness {
    /// `InitFeeSchedule` with an all-zero (zero-fee) tier table. Returns the
    /// fee-schedule PDA. Admin = payer.
    pub fn init_fee_schedule(&mut self) -> Pubkey {
        let fee_schedule = self.pda(&[SEED_FEE_SCHEDULE]).0;
        let tiers = std::array::from_fn(|_| FeeTier {
            volume_threshold: 0,
            fee_bps: 0,
        });
        let ix = self.ix(
            InstructionTag::InitFeeSchedule,
            &InitFeeScheduleArgs { tiers },
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.config_pda(), false),
                AccountMeta::new(fee_schedule, false),
                AccountMeta::new_readonly(system_program::ID, false),
            ],
        );
        self.send(&[ix], &[]).expect("init_fee_schedule");
        fee_schedule
    }

    /// The instrument PDA for a parameter set.
    pub fn instrument_pda(&self, p: &InstrumentParams) -> Pubkey {
        self.pda(&[
            SEED_INSTRUMENT,
            &[p.source],
            &p.market_id,
            &p.leverage_bps.to_le_bytes(),
            &p.collateral_bucket.to_le_bytes(),
            &p.twap_window_slots.to_le_bytes(),
        ])
        .0
    }

    /// The intent-book PDA for an instrument.
    pub fn book_pda(&self, instrument: &Pubkey) -> Pubkey {
        self.pda(&[SEED_BOOK, instrument.as_ref()]).0
    }

    /// The per-market `[SEED_MARKET_NONCE, market_id]` PDA.
    pub fn market_nonce_pda(&self, market_id: &[u8; 32]) -> Pubkey {
        self.pda(&[SEED_MARKET_NONCE, market_id]).0
    }

    /// `CreateInstrument` — admin (payer) creates an instrument + its intent
    /// book. Returns `(instrument PDA, book PDA)`.
    pub fn create_instrument(&mut self, mint: &Pubkey, p: &InstrumentParams) -> (Pubkey, Pubkey) {
        let instrument = self.instrument_pda(p);
        let book = self.book_pda(&instrument);
        let args = CreateInstrumentArgs {
            source: p.source,
            market_id: p.market_id,
            source_token_id_a: [0u8; 32],
            source_token_id_b: [0u8; 32],
            leverage_bps: p.leverage_bps,
            collateral_bucket: p.collateral_bucket,
            twap_window_slots: p.twap_window_slots,
            tick_fp: p.tick_fp,
            liquidation_bps: p.liquidation_bps,
            liquidation_bounty_bps: p.liquidation_bounty_bps,
            max_staleness_secs: p.max_staleness_secs,
            initial_book_capacity: p.initial_book_capacity,
        };
        let ix = self.ix(
            InstructionTag::CreateInstrument,
            &args,
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.config_pda(), false),
                AccountMeta::new(instrument, false),
                AccountMeta::new(book, false),
                AccountMeta::new_readonly(*mint, false),
                AccountMeta::new_readonly(system_program::ID, false),
                AccountMeta::new(self.market_nonce_pda(&p.market_id), false),
            ],
        );
        self.send(&[ix], &[]).expect("create_instrument");
        (instrument, book)
    }

    /// The next intent id the book will assign — read it before a `PostIntent`
    /// to learn the id that post will receive, or before a match to learn the
    /// PMLC id.
    pub fn book_next_intent_id(&self, book: &Pubkey) -> u64 {
        let acct = self.account(book).expect("book account exists");
        let header: &IntentBookHeader =
            bytemuck::from_bytes(&acct.data[..INTENT_BOOK_HEADER_LEN]);
        header.next_intent_id
    }

    /// The book's current node-pool capacity.
    pub fn book_capacity(&self, book: &Pubkey) -> u32 {
        let acct = self.account(book).expect("book account exists");
        let header: &IntentBookHeader =
            bytemuck::from_bytes(&acct.data[..INTENT_BOOK_HEADER_LEN]);
        header.capacity
    }

    /// `ExpandIntentBook` — grow the book by `additional_nodes` slots.
    /// Solana caps account growth at ~10 KiB per transaction, so a
    /// caller wanting a deep book loops this with chunks of ~100 nodes.
    pub fn expand_intent_book(
        &mut self,
        instrument: &Pubkey,
        book: &Pubkey,
        additional_nodes: u32,
    ) -> TxResult {
        let ix = self.ix(
            InstructionTag::ExpandIntentBook,
            &ExpandIntentBookArgs { additional_nodes },
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.config_pda(), false),
                AccountMeta::new_readonly(*instrument, false),
                AccountMeta::new(*book, false),
                AccountMeta::new_readonly(system_program::ID, false),
            ],
        );
        self.send(&[ix], &[])
    }

    /// `PostIntent` (no inline match). Returns the transaction result.
    #[allow(clippy::too_many_arguments)]
    /// Submit a transaction with an explicit compute-unit limit (a
    /// `ComputeBudget` instruction prepended), so a measurement can run
    /// past the default 200k per-instruction ceiling up to 1.4M.
    pub fn send_metered(
        &mut self,
        ixs: &[Instruction],
        extra_signers: &[&Keypair],
        cu_limit: u32,
    ) -> TxResult {
        let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(cu_limit)];
        all.extend_from_slice(ixs);
        self.send(&all, extra_signers)
    }

    /// Build a `PostIntent` instruction (no inline match).
    #[allow(clippy::too_many_arguments)]
    pub fn post_intent_ix(
        &self,
        owner: &Keypair,
        instrument: &Pubkey,
        book: &Pubkey,
        mint: &Pubkey,
        side: u8,
        min_price_fp: u64,
        max_price_fp: u64,
        contracts: u16,
        expiration_slot: u64,
    ) -> Instruction {
        let args = PostIntentArgs {
            side,
            min_price_fp,
            max_price_fp,
            contracts,
            expiration_slot,
            reentry_enabled: 0,
            try_match: 0,
            max_pairs: 0,
        };
        self.ix(
            InstructionTag::PostIntent,
            &args,
            vec![
                AccountMeta::new_readonly(owner.pubkey(), true),
                AccountMeta::new_readonly(self.config_pda(), false),
                AccountMeta::new_readonly(*instrument, false),
                AccountMeta::new(*book, false),
                AccountMeta::new(self.margin_pda(&owner.pubkey(), mint), false),
            ],
        )
    }

    /// `PostIntent` (no inline match). Returns the transaction result.
    #[allow(clippy::too_many_arguments)]
    pub fn post_intent(
        &mut self,
        owner: &Keypair,
        instrument: &Pubkey,
        book: &Pubkey,
        mint: &Pubkey,
        side: u8,
        min_price_fp: u64,
        max_price_fp: u64,
        contracts: u16,
        expiration_slot: u64,
    ) -> TxResult {
        let ix = self.post_intent_ix(
            owner,
            instrument,
            book,
            mint,
            side,
            min_price_fp,
            max_price_fp,
            contracts,
            expiration_slot,
        );
        self.send(&[ix], &[owner])
    }

    /// The `[SEED_USER_VOLUME, owner, mint]` PDA.
    pub fn user_volume_pda(&self, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        self.pda(&[SEED_USER_VOLUME, owner.as_ref(), mint.as_ref()]).0
    }

    /// The `[SEED_TREASURY, mint]` fee-treasury PDA.
    pub fn fee_treasury_pda(&self, mint: &Pubkey) -> Pubkey {
        self.pda(&[SEED_TREASURY, mint.as_ref()]).0
    }

    /// Build a `MatchPair` instruction. Returns `(instruction, PMLC PDA)`.
    #[allow(clippy::too_many_arguments)]
    pub fn match_pair_ix(
        &self,
        instrument: &Pubkey,
        book: &Pubkey,
        mint: &Pubkey,
        long_owner: &Pubkey,
        short_owner: &Pubkey,
        long_id: u64,
        short_id: u64,
        taker: &Pubkey,
        maker: &Pubkey,
    ) -> (Instruction, Pubkey) {
        let pmlc_id = self.book_next_intent_id(book);
        let (pmlc, _) = Pmlc::find_pda(&self.program_id, instrument, pmlc_id);
        let ix = self.ix(
            InstructionTag::MatchPair,
            &MatchPairArgs {
                long_intent_id: long_id,
                short_intent_id: short_id,
                max_contracts: 1,
            },
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(*instrument, false),
                AccountMeta::new(*book, false),
                AccountMeta::new(self.margin_pda(long_owner, mint), false),
                AccountMeta::new(self.margin_pda(short_owner, mint), false),
                AccountMeta::new(pmlc, false),
                AccountMeta::new_readonly(system_program::ID, false),
                AccountMeta::new_readonly(self.pda(&[SEED_FEE_SCHEDULE]).0, false),
                AccountMeta::new(self.user_volume_pda(taker, mint), false),
                AccountMeta::new(self.user_volume_pda(maker, mint), false),
                AccountMeta::new(self.fee_treasury_pda(mint), false),
            ],
        );
        (ix, pmlc)
    }

    /// `MatchPair` — match an explicit `(long_id, short_id)` pair into a new
    /// PMLC. `taker`/`maker` are the volume-account owners (taker = the side
    /// posted later). Returns `(tx result, PMLC PDA)`.
    #[allow(clippy::too_many_arguments)]
    pub fn match_pair(
        &mut self,
        instrument: &Pubkey,
        book: &Pubkey,
        mint: &Pubkey,
        long_owner: &Pubkey,
        short_owner: &Pubkey,
        long_id: u64,
        short_id: u64,
        taker: &Pubkey,
        maker: &Pubkey,
    ) -> (TxResult, Pubkey) {
        let (ix, pmlc) = self.match_pair_ix(
            instrument,
            book,
            mint,
            long_owner,
            short_owner,
            long_id,
            short_id,
            taker,
            maker,
        );
        (self.send(&[ix], &[]), pmlc)
    }

    /// Load + copy a PMLC account's state.
    pub fn load_pmlc(&self, pmlc: &Pubkey) -> Pmlc {
        let acct = self.account(pmlc).expect("pmlc account exists");
        *Pmlc::load(&acct.data).expect("pmlc loads")
    }
}

// --- Admin: global pause + governance timelock ------------------------------

impl Harness {
    /// `SetGlobalPause` — admin (payer) toggles the global pause flag.
    /// The payload is a single raw byte, not Borsh.
    pub fn set_global_pause(&mut self, paused: bool) {
        let ix = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(self.config_pda(), false),
            ],
            data: vec![InstructionTag::SetGlobalPause as u8, paused as u8],
        };
        self.send(&[ix], &[]).expect("set_global_pause");
    }

    /// Load + copy the singleton `ProgramConfig`.
    pub fn load_program_config(&self) -> ProgramConfig {
        let acct = self.account(&self.config_pda()).expect("config exists");
        *ProgramConfig::load(&acct.data).expect("config loads")
    }

    /// The `[SEED_TIMELOCK, proposal_id]` proposal PDA.
    pub fn timelock_pda(&self, proposal_id: u64) -> Pubkey {
        self.pda(&[SEED_TIMELOCK, &proposal_id.to_le_bytes()]).0
    }

    /// `ProposeSetAttestationSigner` — admin opens a signer-rotation
    /// proposal. Returns the proposal PDA.
    pub fn propose_set_attestation_signer(
        &mut self,
        proposal_id: u64,
        new_signer: [u8; 32],
    ) -> Pubkey {
        let tl = self.timelock_pda(proposal_id);
        let ix = self.ix(
            InstructionTag::ProposeSetAttestationSigner,
            &ProposeSetAttestationSignerArgs {
                proposal_id,
                new_signer,
            },
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.config_pda(), false),
                AccountMeta::new(tl, false),
                AccountMeta::new_readonly(system_program::ID, false),
            ],
        );
        self.send(&[ix], &[])
            .expect("propose_set_attestation_signer");
        tl
    }

    /// `ExecuteSetAttestationSigner` — execute a matured proposal.
    pub fn execute_set_attestation_signer(&mut self, proposal_id: u64) -> TxResult {
        let ix = self.ix(
            InstructionTag::ExecuteSetAttestationSigner,
            &ExecuteTimelockArgs { proposal_id },
            vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(self.config_pda(), false),
                AccountMeta::new(self.timelock_pda(proposal_id), false),
            ],
        );
        self.send(&[ix], &[])
    }

    /// `CancelTimelockProposal` — cancel a pending proposal.
    pub fn cancel_timelock(&mut self, proposal_id: u64) -> TxResult {
        let ix = self.ix(
            InstructionTag::CancelTimelockProposal,
            &CancelTimelockArgs { proposal_id },
            vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(self.timelock_pda(proposal_id), false),
            ],
        );
        self.send(&[ix], &[])
    }

    /// Advance the cluster clock's unix timestamp by `secs` — used to
    /// mature governance timelocks instantly. litesvm lets the `Clock`
    /// sysvar be set directly; no validator wait.
    pub fn warp_unix(&mut self, secs: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp += secs;
        self.svm.set_sysvar(&clock);
    }
}

// --- Position lifecycle: novation + substitution ----------------------------

impl Harness {
    /// `Novate` — transfer one `side` of a live PMLC from `current_owner`
    /// to `new_owner` (who must have a margin account with free ≥ the
    /// per-side collateral).
    #[allow(clippy::too_many_arguments)]
    pub fn novate(
        &mut self,
        instrument: &Pubkey,
        pmlc: &Pubkey,
        mint: &Pubkey,
        side: u8,
        current_owner: &Keypair,
        new_owner: &Keypair,
    ) -> TxResult {
        let ix = self.ix(
            InstructionTag::Novate,
            &NovateArgs { side },
            vec![
                AccountMeta::new_readonly(current_owner.pubkey(), true),
                AccountMeta::new_readonly(new_owner.pubkey(), true),
                AccountMeta::new_readonly(self.config_pda(), false),
                AccountMeta::new_readonly(*instrument, false),
                AccountMeta::new(*pmlc, false),
                AccountMeta::new(self.margin_pda(&current_owner.pubkey(), mint), false),
                AccountMeta::new(self.margin_pda(&new_owner.pubkey(), mint), false),
            ],
        );
        self.send(&[ix], &[current_owner, new_owner])
    }

    /// `MatchSubstituteWithSettle` — `substitutor` exits their PMLC side
    /// against a fresh intent pair; `counterparty` (the other party to
    /// those intents) takes the side over, with on-chain PnL settled at
    /// the match midpoint.
    #[allow(clippy::too_many_arguments)]
    pub fn match_substitute_with_settle(
        &mut self,
        instrument: &Pubkey,
        book: &Pubkey,
        pmlc: &Pubkey,
        mint: &Pubkey,
        substitutor: &Keypair,
        counterparty: &Pubkey,
        long_id: u64,
        short_id: u64,
    ) -> TxResult {
        let ix = self.ix(
            InstructionTag::MatchSubstituteWithSettle,
            &MatchPairArgs {
                long_intent_id: long_id,
                short_intent_id: short_id,
                max_contracts: 1,
            },
            vec![
                AccountMeta::new_readonly(substitutor.pubkey(), true),
                AccountMeta::new_readonly(*instrument, false),
                AccountMeta::new(*book, false),
                AccountMeta::new(*pmlc, false),
                AccountMeta::new(self.margin_pda(&substitutor.pubkey(), mint), false),
                AccountMeta::new(self.margin_pda(counterparty, mint), false),
            ],
        );
        self.send(&[ix], &[substitutor])
    }
}

// --- Settlement: liquidate, resolve, close ----------------------------------

impl Harness {
    /// The current cluster unix timestamp.
    pub fn now_unix(&self) -> i64 {
        let clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp
    }

    /// Load + copy a margin account's state.
    pub fn load_margin(&self, owner: &Pubkey, mint: &Pubkey) -> MarginAccount {
        let acct = self
            .account(&self.margin_pda(owner, mint))
            .expect("margin account exists");
        *MarginAccount::load(&acct.data).expect("margin loads")
    }

    /// Build the bare `Liquidate` instruction (no attestation ix). Used
    /// by adversarial tests that omit or tamper with the attestation.
    #[allow(clippy::too_many_arguments)]
    pub fn liquidate_ix(
        &self,
        instrument: &Pubkey,
        pmlc: &Pubkey,
        mint: &Pubkey,
        market_id: [u8; 32],
        keeper: &Pubkey,
        long_owner: &Pubkey,
        short_owner: &Pubkey,
    ) -> Instruction {
        self.ix(
            InstructionTag::Liquidate,
            &LiquidateArgs {
                attestation_ix_offset: 0,
            },
            vec![
                AccountMeta::new(*keeper, true),
                AccountMeta::new_readonly(self.config_pda(), false),
                AccountMeta::new_readonly(sysvar_instructions::ID, false),
                AccountMeta::new_readonly(*instrument, false),
                AccountMeta::new(*pmlc, false),
                AccountMeta::new(self.market_nonce_pda(&market_id), false),
                AccountMeta::new(self.margin_pda(long_owner, mint), false),
                AccountMeta::new(self.margin_pda(short_owner, mint), false),
                AccountMeta::new(self.margin_pda(keeper, mint), false),
            ],
        )
    }

    /// `Liquidate` with a caller-supplied attestation: `signer` produces
    /// the Ed25519 precompile ix over `attestation` (whatever bytes the
    /// caller crafted). Adversarial tests use this to forge signers,
    /// types, PMLC bindings, nonces, etc.
    #[allow(clippy::too_many_arguments)]
    pub fn liquidate_signed(
        &mut self,
        signer: &Attestor,
        attestation: &[u8],
        instrument: &Pubkey,
        pmlc: &Pubkey,
        mint: &Pubkey,
        market_id: [u8; 32],
        keeper: &Keypair,
        long_owner: &Pubkey,
        short_owner: &Pubkey,
    ) -> TxResult {
        let ed_ix = signer.ed25519_verify_ix(attestation);
        let liq_ix =
            self.liquidate_ix(instrument, pmlc, mint, market_id, &keeper.pubkey(), long_owner, short_owner);
        self.send(&[ed_ix, liq_ix], &[keeper])
    }

    /// `Liquidate` — settle an underwater PMLC at a historical breach
    /// mark, with a well-formed attestation signed by `attestor`.
    #[allow(clippy::too_many_arguments)]
    pub fn liquidate(
        &mut self,
        attestor: &Attestor,
        instrument: &Pubkey,
        pmlc: &Pubkey,
        mint: &Pubkey,
        market_id: [u8; 32],
        keeper: &Keypair,
        long_owner: &Pubkey,
        short_owner: &Pubkey,
        breach_mark_fp: u64,
        nonce: u64,
    ) -> TxResult {
        let now = self.now_unix();
        let att = attestor.historical_liquidation(
            market_id,
            now.max(0) as u64,
            nonce,
            pmlc.to_bytes(),
            breach_mark_fp,
            now,
        );
        self.liquidate_signed(
            attestor,
            &att,
            instrument,
            pmlc,
            mint,
            market_id,
            keeper,
            long_owner,
            short_owner,
        )
    }

    /// Build the bare `Resolve` instruction (no attestation ix).
    pub fn resolve_ix(
        &self,
        instrument: &Pubkey,
        pmlc: &Pubkey,
        mint: &Pubkey,
        market_id: [u8; 32],
        long_owner: &Pubkey,
        short_owner: &Pubkey,
    ) -> Instruction {
        self.ix(
            InstructionTag::Resolve,
            &ResolveArgs {
                attestation_ix_offset: 0,
            },
            vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.config_pda(), false),
                AccountMeta::new_readonly(sysvar_instructions::ID, false),
                AccountMeta::new_readonly(*instrument, false),
                AccountMeta::new(*pmlc, false),
                AccountMeta::new(self.market_nonce_pda(&market_id), false),
                AccountMeta::new(self.margin_pda(long_owner, mint), false),
                AccountMeta::new(self.margin_pda(short_owner, mint), false),
            ],
        )
    }

    /// `Resolve` with a caller-supplied attestation, for adversarial tests.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_signed(
        &mut self,
        signer: &Attestor,
        attestation: &[u8],
        instrument: &Pubkey,
        pmlc: &Pubkey,
        mint: &Pubkey,
        market_id: [u8; 32],
        long_owner: &Pubkey,
        short_owner: &Pubkey,
    ) -> TxResult {
        let ed_ix = signer.ed25519_verify_ix(attestation);
        let res_ix = self.resolve_ix(instrument, pmlc, mint, market_id, long_owner, short_owner);
        self.send(&[ed_ix, res_ix], &[])
    }

    /// `Resolve` — settle a PMLC at market resolution with a well-formed
    /// attestation signed by `attestor`. Caller = payer.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &mut self,
        attestor: &Attestor,
        instrument: &Pubkey,
        pmlc: &Pubkey,
        mint: &Pubkey,
        market_id: [u8; 32],
        long_owner: &Pubkey,
        short_owner: &Pubkey,
        final_outcome_bps: u16,
        nonce: u64,
    ) -> TxResult {
        let now = self.now_unix();
        let att = attestor.resolution(
            market_id,
            now.max(0) as u64,
            nonce,
            final_outcome_bps,
            now.max(0) as u64,
        );
        self.resolve_signed(
            attestor,
            &att,
            instrument,
            pmlc,
            mint,
            market_id,
            long_owner,
            short_owner,
        )
    }

    /// `ClosePmlc` — close a settled PMLC, refunding its rent to the payer.
    pub fn close_pmlc(&mut self, pmlc: &Pubkey) -> TxResult {
        let ix = self.ix_bare(
            InstructionTag::ClosePmlc,
            vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(*pmlc, false),
            ],
        );
        self.send(&[ix], &[])
    }
}
