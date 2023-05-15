use std::fmt::{Debug, Formatter};
use solana_ledger::shred::Shred;
use solana_sdk::clock::Slot;

// tuple (slot, fec_set_index) ErasureSetId: see shred.rs: Tuple which identifies erasure coding set that the shred belongs to
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct ErasureSetId(Slot, /*fec_set_index:*/ u32);

impl From<&Shred> for ErasureSetId {
    fn from(s: &Shred) -> Self {
        ErasureSetId(s.slot(), s.fec_set_index())
    }
}

impl Debug for ErasureSetId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "slot {} fec_set_index {}", self.0, self.1)
    }
}

