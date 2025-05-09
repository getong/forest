// Copyright 2019-2025 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use super::errors::Error;
use crate::rpc::eth::types::EthAddress;
use crate::shim::{
    address::Address,
    crypto::{Signature, SignatureType},
};
use crate::utils::encoding::{blake2b_256, keccak_256};
use bls_signatures::{PrivateKey as BlsPrivate, Serialize};
use libsecp256k1::{Message as SecpMessage, PublicKey as SecpPublic, SecretKey as SecpPrivate};

/// Return the public key for a given private key and [`SignatureType`]
pub fn to_public(sig_type: SignatureType, private_key: &[u8]) -> Result<Vec<u8>, Error> {
    match sig_type {
        SignatureType::Bls => Ok(BlsPrivate::from_bytes(private_key)
            .map_err(|err| Error::Other(err.to_string()))?
            .public_key()
            .as_bytes()),
        SignatureType::Secp256k1 => {
            let private_key = SecpPrivate::parse_slice(private_key)
                .map_err(|err| Error::Other(err.to_string()))?;
            let public_key = SecpPublic::from_secret_key(&private_key);
            Ok(public_key.serialize().to_vec())
        }
        SignatureType::Delegated => {
            let private_key = SecpPrivate::parse_slice(private_key)
                .map_err(|err| Error::Other(err.to_string()))?;
            let public_key = SecpPublic::from_secret_key(&private_key);
            Ok(public_key.serialize().to_vec())
        }
    }
}

/// Return a new Address that is of a given [`SignatureType`] and uses the
/// supplied public key
pub fn new_address(sig_type: SignatureType, public_key: &[u8]) -> Result<Address, Error> {
    match sig_type {
        SignatureType::Bls => {
            let addr = Address::new_bls(public_key).map_err(|err| Error::Other(err.to_string()))?;
            Ok(addr)
        }
        SignatureType::Secp256k1 => {
            let addr =
                Address::new_secp256k1(public_key).map_err(|err| Error::Other(err.to_string()))?;
            Ok(addr)
        }
        SignatureType::Delegated => {
            let eth_addr = EthAddress::eth_address_from_pub_key(public_key)
                .map_err(|err| Error::Other(err.to_string()))?;
            let addr = eth_addr
                .to_filecoin_address()
                .map_err(|err| Error::Other(err.to_string()))?;
            Ok(addr)
        }
    }
}

/// Sign takes in [`SignatureType`], private key and message. Returns a Signature
/// for that message
pub fn sign(sig_type: SignatureType, private_key: &[u8], msg: &[u8]) -> Result<Signature, Error> {
    match sig_type {
        SignatureType::Bls => {
            let priv_key =
                BlsPrivate::from_bytes(private_key).map_err(|err| Error::Other(err.to_string()))?;
            // this returns a signature from bls-signatures, so we need to convert this to a
            // crypto signature
            let sig = priv_key.sign(msg);
            let crypto_sig = Signature::new_bls(sig.as_bytes());
            Ok(crypto_sig)
        }
        SignatureType::Secp256k1 => {
            let priv_key = SecpPrivate::parse_slice(private_key)
                .map_err(|err| Error::Other(err.to_string()))?;
            let msg_hash = blake2b_256(msg);
            let message = SecpMessage::parse(&msg_hash);
            let (sig, recovery_id) = libsecp256k1::sign(&message, &priv_key);
            let mut new_bytes = [0; 65];
            new_bytes[..64].copy_from_slice(&sig.serialize());
            new_bytes[64] = recovery_id.serialize();
            let crypto_sig = Signature::new_secp256k1(new_bytes.to_vec());
            Ok(crypto_sig)
        }
        SignatureType::Delegated => {
            let priv_key = SecpPrivate::parse_slice(private_key)
                .map_err(|err| Error::Other(err.to_string()))?;

            let msg_hash = keccak_256(msg);
            let message = SecpMessage::parse(&msg_hash);
            let (sig, recovery_id) = libsecp256k1::sign(&message, &priv_key);
            let mut new_bytes = [0; 65];
            new_bytes[..64].copy_from_slice(&sig.serialize());
            new_bytes[64] = recovery_id.serialize();
            let crypto_sig = Signature::new_delegated(new_bytes.to_vec());
            Ok(crypto_sig)
        }
    }
}

/// Generate a new private key
pub fn generate(sig_type: SignatureType) -> Result<Vec<u8>, Error> {
    let rng = &mut crate::utils::rand::forest_os_rng();
    match sig_type {
        SignatureType::Bls => {
            let key = BlsPrivate::generate(rng);
            Ok(key.as_bytes())
        }
        SignatureType::Secp256k1 => {
            let key = SecpPrivate::random(rng);
            Ok(key.serialize().to_vec())
        }
        SignatureType::Delegated => {
            let key = SecpPrivate::random(rng);
            Ok(key.serialize().to_vec())
        }
    }
}
