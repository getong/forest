// Copyright 2019-2025 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use crate::shim::address::Address;
use blake2b_simd::Params;
use fil_actors_shared::filecoin_proofs_api::ProverId;
use fvm_ipld_encoding::strict_bytes::{Deserialize, Serialize};
use serde::{Deserializer, Serializer, de, ser};

mod fallback_de_ipld_dagcbor;

/// This method will attempt to de-serialize given bytes using the regular
/// `serde_ipld_dagcbor::from_slice`. Due to a historical issue in Lotus (see more in
/// [FIP-0027](https://github.com/filecoin-project/FIPs/blob/master/FIPS/fip-0027.md), we must still
/// support strings with invalid UTF-8 bytes. On a failure, it
/// will retry the operation using the fallback that will de-serialize
/// strings with invalid UTF-8 bytes as bytes.
pub fn from_slice_with_fallback<'a, T: serde::de::Deserialize<'a>>(
    bytes: &'a [u8],
) -> anyhow::Result<T> {
    match serde_ipld_dagcbor::from_slice(bytes) {
        Ok(v) => Ok(v),
        Err(err) => fallback_de_ipld_dagcbor::from_slice(bytes).map_err(|fallback_err| {
            anyhow::anyhow!(
                "Fallback deserialization failed: {fallback_err}. Original error: {err}"
            )
        }),
    }
}

mod cid_de_cbor;
pub use cid_de_cbor::extract_cids;

/// `serde_bytes` with max length check
pub mod serde_byte_array {
    use super::*;
    /// lotus use cbor-gen for generating codec for types, it has a length limit
    /// for byte array as `2 << 20`
    ///
    /// <https://github.com/whyrusleeping/cbor-gen/blob/f57984553008dd4285df16d4ec2760f97977d713/gen.go#L16>
    pub const BYTE_ARRAY_MAX_LEN: usize = 2 << 20;

    /// checked if `input > crate::utils::BYTE_ARRAY_MAX_LEN`
    pub fn serialize<T, S>(bytes: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: ?Sized + Serialize + AsRef<[u8]>,
        S: Serializer,
    {
        let len = bytes.as_ref().len();
        if len > BYTE_ARRAY_MAX_LEN {
            return Err(ser::Error::custom::<String>(
                "Array exceed max length".into(),
            ));
        }

        Serialize::serialize(bytes, serializer)
    }

    /// checked if `output > crate::utils::ByteArrayMaxLen`
    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        T: Deserialize<'de> + AsRef<[u8]>,
        D: Deserializer<'de>,
    {
        Deserialize::deserialize(deserializer).and_then(|bytes: T| {
            if bytes.as_ref().len() > BYTE_ARRAY_MAX_LEN {
                Err(de::Error::custom::<String>(
                    "Array exceed max length".into(),
                ))
            } else {
                Ok(bytes)
            }
        })
    }
}

/// Generates BLAKE2b hash of fixed 32 bytes size.
///
/// # Example
/// ```
/// # use forest::doctest_private::blake2b_256;
///
/// let ingest: Vec<u8> = vec![];
/// let hash = blake2b_256(&ingest);
/// assert_eq!(hash.len(), 32);
/// ```
pub fn blake2b_256(ingest: &[u8]) -> [u8; 32] {
    let digest = Params::new()
        .hash_length(32)
        .to_state()
        .update(ingest)
        .finalize();

    let mut ret = [0u8; 32];
    ret.clone_from_slice(digest.as_bytes());
    ret
}

/// Generates Keccak-256 hash of fixed 32 bytes size.
///
/// # Example
/// ```
/// # use forest::doctest_private::keccak_256;
/// let ingest: Vec<u8> = vec![];
/// let hash = keccak_256(&ingest);
/// assert_eq!(hash.len(), 32);
/// ```
pub fn keccak_256(ingest: &[u8]) -> [u8; 32] {
    let mut ret: [u8; 32] = Default::default();
    keccak_hash::keccak_256(ingest, &mut ret);
    ret
}

pub fn prover_id_from_u64(id: u64) -> ProverId {
    let mut prover_id = ProverId::default();
    let prover_bytes = Address::new_id(id).payload().to_raw_bytes();
    assert!(prover_bytes.len() <= prover_id.len());
    #[allow(clippy::indexing_slicing)]
    prover_id[..prover_bytes.len()].copy_from_slice(&prover_bytes);
    prover_id
}

#[cfg(test)]
mod tests {
    use ipld_core::ipld::Ipld;
    use itertools::Itertools as _;
    use rand::Rng;
    use serde::{Deserialize, Serialize};
    use serde_ipld_dagcbor::to_vec;

    use super::*;
    use crate::utils::encoding::serde_byte_array::BYTE_ARRAY_MAX_LEN;

    #[test]
    fn vector_hashing() {
        let ing_vec = vec![1, 2, 3];

        assert_eq!(blake2b_256(&ing_vec), blake2b_256(&[1, 2, 3]));
        assert_ne!(blake2b_256(&ing_vec), blake2b_256(&[1, 2, 3, 4]));
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct ByteArray {
        #[serde(with = "serde_byte_array")]
        pub inner: Vec<u8>,
    }

    #[test]
    fn can_serialize_byte_array() {
        for len in [0, 1, BYTE_ARRAY_MAX_LEN] {
            let bytes = ByteArray {
                inner: vec![0; len],
            };

            assert!(serde_ipld_dagcbor::to_vec(&bytes).is_ok());
        }
    }

    #[test]
    fn cannot_serialize_byte_array_overflow() {
        let bytes = ByteArray {
            inner: vec![0; BYTE_ARRAY_MAX_LEN + 1],
        };

        let err = serde_ipld_dagcbor::to_vec(&bytes).unwrap_err();
        assert!(
            format!("{err}").contains("Struct value cannot be serialized."),
            "{err}"
        );
    }

    #[test]
    fn can_deserialize_byte_array() {
        for len in [0, 1, BYTE_ARRAY_MAX_LEN] {
            let bytes = ByteArray {
                inner: vec![0; len],
            };

            let encoding = serde_ipld_dagcbor::to_vec(&bytes).unwrap();
            assert_eq!(
                from_slice_with_fallback::<ByteArray>(&encoding).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn cannot_deserialize_byte_array_overflow() {
        let max_length_bytes = ByteArray {
            inner: vec![0; BYTE_ARRAY_MAX_LEN],
        };

        // prefix: 2 ^ 21 -> 2 ^ 21 + 1
        let mut overflow_encoding = serde_ipld_dagcbor::to_vec(&max_length_bytes).unwrap();
        let encoding_len = overflow_encoding.len();
        overflow_encoding[encoding_len - BYTE_ARRAY_MAX_LEN - 1] = 1;
        overflow_encoding.push(0);

        assert!(
            format!(
                "{}",
                from_slice_with_fallback::<ByteArray>(&overflow_encoding)
                    .err()
                    .unwrap()
            )
            .contains("Array exceed max length")
        );
    }

    #[test]
    fn parity_tests() {
        use cs_serde_bytes;

        #[derive(Deserialize, Serialize)]
        struct A(#[serde(with = "fvm_ipld_encoding::strict_bytes")] Vec<u8>);

        #[derive(Deserialize, Serialize)]
        struct B(#[serde(with = "cs_serde_bytes")] Vec<u8>);

        let mut array = [0; 1024];
        crate::utils::rand::forest_rng().fill(&mut array);

        let a = A(array.to_vec());
        let b = B(array.to_vec());

        assert_eq!(
            serde_json::to_string_pretty(&a).unwrap(),
            serde_json::to_string_pretty(&b).unwrap()
        );
    }

    #[test]
    fn test_fallback_deserialization() {
        // where the regular deserialization fails with invalid UTF-8 strings, the fallback should
        // succeed.

        // Valid UTF-8, should return the same results.
        let ipld_string = Ipld::String("cthulhu".to_string());
        let serialized = to_vec(&ipld_string).unwrap();
        assert_eq!(
            ipld_string,
            serde_ipld_dagcbor::from_slice::<Ipld>(&serialized).unwrap()
        );
        assert_eq!(
            ipld_string,
            from_slice_with_fallback::<Ipld>(&serialized).unwrap()
        );

        // Invalid UTF-8, regular deserialization fails, fallback succeeds. We can
        // extract the bytes.
        let corrupted = serialized
            .iter()
            .take(serialized.len() - 2)
            .chain(&[0xa0, 0xa1])
            .copied()
            .collect_vec();
        assert!(
            matches!(from_slice_with_fallback::<Ipld>(&corrupted).unwrap(), Ipld::Bytes(bytes) if bytes == [0x63, 0x74, 0x68, 0x75, 0x6c, 0xa0, 0xa1])
        )
    }
}
