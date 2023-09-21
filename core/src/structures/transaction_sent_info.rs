use solana_sdk::slot_history::Slot;

pub type WireTransaction = Vec<u8>;

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct SentTransactionInfo {
    pub signature: String,
    // not useful?
    pub slot: Slot,
    pub transaction: WireTransaction,
    // use for data_cache only
    pub last_valid_block_height: u64,
}
