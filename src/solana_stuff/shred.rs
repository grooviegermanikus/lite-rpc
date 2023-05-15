
// logic copied from solana shred.rs

use solana_ledger::shred::{Error, ReedSolomonCache, Shred, self, ShredType};



// const OFFSET_OF_SHRED_VARIANT: usize = 64;
//
// // Simple
// #[derive(Clone, Copy, Debug)]
// enum ShredVariantSimple {
//     LegacyCode,
//     LegacyData,
//     MerkleCode,
//     MerkleData,
// }
//
// fn get_shred_variant(shred: &[u8]) -> Result<ShredVariantSimple, Error> {
//     let shred_variant: u8 = match shred.get(OFFSET_OF_SHRED_VARIANT) {
//         None => return Err(Error::InvalidPayloadSize(shred.len())),
//         Some(shred_variant) => *shred_variant,
//     };
//
//
//
// }
//
//
// pub fn recover(
//     shreds: Vec<Shred>,
//     reed_solomon_cache: &ReedSolomonCache,
// ) -> Result<Vec<Shred>, Error> {
//     let sssss: &Shred = shreds.get(0).unwrap();
//     match get_shred_variant(shreds.to_bytes())?
//     {
//         ShredVariantSimple::LegacyData | ShredVariantSimple::LegacyCode => {
//             solana_ledger::shred::Shredder::try_recovery(shreds, reed_solomon_cache)
//         }
//         ShredVariantSimple::MerkleCode | ShredVariantSimple::MerkleData => {
//             let shreds = shreds
//                 .into_iter()
//                 .map(solana_ledger::shred::merkle::Shred::try_from)
//                 .collect::<Result<_, _>>()?;
//             Ok(merkle::recover(shreds, reed_solomon_cache)?
//                 .into_iter()
//                 .map(Shred::from)
//                 .collect())
//         }
//     }
// }
//
// impl TryFrom<u8> for ShredVariant {
//     type Error = Error;
//     fn try_from(shred_variant: u8) -> Result<Self, Self::Error> {
//         if shred_variant == u8::from(ShredType::Code) {
//             Ok(ShredVariant::LegacyCode)
//         } else if shred_variant == u8::from(ShredType::Data) {
//             Ok(ShredVariant::LegacyData)
//         } else {
//             match shred_variant & 0xF0 {
//                 0x40 => Ok(ShredVariant::MerkleCode(shred_variant & 0x0F)),
//                 0x80 => Ok(ShredVariant::MerkleData(shred_variant & 0x0F)),
//                 _ => panic!("not handled"),
//             }
//         }
//     }
// }
