//! Simulated CRE/TEE attestor.
//!
//! In production the polyleverage program settles against attestations
//! signed by a TEE-held Ed25519 key registered in
//! `ProgramConfig.attestation_signer`. The harness stands in for that
//! TEE: a local Ed25519 keypair frames + signs attestations in the
//! exact 104-byte layout `polyleverage::attestation` parses, and emits
//! the `Ed25519SigVerify` precompile instruction the settlement
//! handlers introspect via the instructions sysvar.
//!
//! Layout constants come from the program crate itself, so the harness
//! cannot drift from the wire format.

use polyleverage::attestation::{
    ATTESTATION_LEN, ATTESTATION_MAGIC, ATTESTATION_PAYLOAD_OFFSET, ATT_TYPE_HISTORICAL_LIQUIDATION,
    ATT_TYPE_PRICE_TWAP, ATT_TYPE_RESOLUTION,
};
use solana_sdk::{
    ed25519_program,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
};

/// Local Ed25519 attestor key + attestation builders.
pub struct Attestor {
    keypair: Keypair,
}

impl Default for Attestor {
    fn default() -> Self {
        Self::new()
    }
}

impl Attestor {
    pub fn new() -> Self {
        Self {
            keypair: Keypair::new(),
        }
    }

    pub fn from_keypair(keypair: Keypair) -> Self {
        Self { keypair }
    }

    pub fn pubkey(&self) -> Pubkey {
        self.keypair.pubkey()
    }

    /// The 32-byte value to pass as `ProgramConfig.attestation_signer`.
    pub fn signer_bytes(&self) -> [u8; 32] {
        self.keypair.pubkey().to_bytes()
    }

    /// Frame the fixed header + market id + timestamp + nonce, then copy
    /// the type-specific payload into the 48-byte payload region.
    fn frame(
        att_type: u8,
        market_id: [u8; 32],
        timestamp: u64,
        nonce: u64,
        payload: &[u8],
    ) -> [u8; ATTESTATION_LEN] {
        let mut b = [0u8; ATTESTATION_LEN];
        b[0..4].copy_from_slice(&ATTESTATION_MAGIC);
        b[4] = att_type;
        b[8..40].copy_from_slice(&market_id);
        b[40..48].copy_from_slice(&timestamp.to_le_bytes());
        b[48..56].copy_from_slice(&nonce.to_le_bytes());
        let n = payload.len().min(ATTESTATION_LEN - ATTESTATION_PAYLOAD_OFFSET);
        b[ATTESTATION_PAYLOAD_OFFSET..ATTESTATION_PAYLOAD_OFFSET + n]
            .copy_from_slice(&payload[..n]);
        b
    }

    /// PRICE_TWAP attestation: `price_fp` + `twap_window_slots`.
    pub fn price_twap(
        &self,
        market_id: [u8; 32],
        timestamp: u64,
        nonce: u64,
        price_fp: u64,
        twap_window_slots: u64,
    ) -> [u8; ATTESTATION_LEN] {
        let mut p = [0u8; 48];
        p[0..8].copy_from_slice(&price_fp.to_le_bytes());
        p[8..16].copy_from_slice(&twap_window_slots.to_le_bytes());
        Self::frame(ATT_TYPE_PRICE_TWAP, market_id, timestamp, nonce, &p)
    }

    /// RESOLUTION attestation: `final_outcome_bps` (0 / 5000 / 10000).
    pub fn resolution(
        &self,
        market_id: [u8; 32],
        timestamp: u64,
        nonce: u64,
        final_outcome_bps: u16,
        resolved_at_ts: u64,
    ) -> [u8; ATTESTATION_LEN] {
        let mut p = [0u8; 48];
        p[0..2].copy_from_slice(&final_outcome_bps.to_le_bytes());
        p[2..10].copy_from_slice(&resolved_at_ts.to_le_bytes());
        Self::frame(ATT_TYPE_RESOLUTION, market_id, timestamp, nonce, &p)
    }

    /// HISTORICAL_LIQUIDATION attestation bound to a specific PMLC.
    pub fn historical_liquidation(
        &self,
        market_id: [u8; 32],
        timestamp: u64,
        nonce: u64,
        pmlc: [u8; 32],
        breach_mark_fp: u64,
        breach_unix_ts: i64,
    ) -> [u8; ATTESTATION_LEN] {
        let mut p = [0u8; 48];
        p[0..32].copy_from_slice(&pmlc);
        p[32..40].copy_from_slice(&breach_mark_fp.to_le_bytes());
        p[40..48].copy_from_slice(&breach_unix_ts.to_le_bytes());
        Self::frame(
            ATT_TYPE_HISTORICAL_LIQUIDATION,
            market_id,
            timestamp,
            nonce,
            &p,
        )
    }

    /// Build the `Ed25519SigVerify` precompile instruction the program
    /// introspects. The settlement instruction must be placed
    /// immediately after this one in the same transaction.
    ///
    /// All three instruction-index fields are `u16::MAX` ("this
    /// instruction"); the program rejects any other value because a
    /// cross-instruction index would let the precompile verify bytes
    /// the program never sees.
    pub fn ed25519_verify_ix(&self, attestation: &[u8]) -> Instruction {
        let signature = self.keypair.sign_message(attestation);
        let pubkey = self.keypair.pubkey();

        const HEADER: usize = 16;
        let sig_off = HEADER as u16;
        let pk_off = (HEADER + 64) as u16;
        let msg_off = (HEADER + 64 + 32) as u16;

        let mut data = vec![0u8; msg_off as usize + attestation.len()];
        data[0] = 1; // num_signatures
        data[1] = 0; // padding
        data[2..4].copy_from_slice(&sig_off.to_le_bytes());
        data[4..6].copy_from_slice(&u16::MAX.to_le_bytes());
        data[6..8].copy_from_slice(&pk_off.to_le_bytes());
        data[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
        data[10..12].copy_from_slice(&msg_off.to_le_bytes());
        data[12..14].copy_from_slice(&(attestation.len() as u16).to_le_bytes());
        data[14..16].copy_from_slice(&u16::MAX.to_le_bytes());
        data[sig_off as usize..sig_off as usize + 64]
            .copy_from_slice(signature.as_ref());
        data[pk_off as usize..pk_off as usize + 32].copy_from_slice(&pubkey.to_bytes());
        data[msg_off as usize..].copy_from_slice(attestation);

        Instruction {
            program_id: ed25519_program::ID,
            accounts: vec![],
            data,
        }
    }
}
