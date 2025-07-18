// Copyright 2019-2025 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT
use super::*;
use fvm_ipld_encoding::RawBytes;
use jsonrpsee::core::Serialize;
use paste::paste;
use schemars::JsonSchema;
use serde::Deserialize;
use std::fmt::Debug;

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct EVMConstructorParamsLotusJson {
    pub creator: [u8; 20],
    #[schemars(with = "LotusJson<RawBytes>")]
    #[serde(with = "crate::lotus_json")]
    pub initcode: RawBytes,
}

macro_rules! impl_evm_constructor_params {
    ($($version:literal),+) => {
        $(
            paste! {
                impl HasLotusJson for fil_actor_evm_state::[<v $version>]::ConstructorParams {
                    type LotusJson = EVMConstructorParamsLotusJson;

                    #[cfg(test)]
                    fn snapshots() -> Vec<(serde_json::Value, Self)> {
                        vec![
                            (
                                json!({
                                        "Creator": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                                        "Initcode": "ESIzRFU="
                                    }),
                                Self {
                                    creator: fil_actor_evm_state::evm_shared::[<v $version>]::address::EthAddress([0; 20]),
                                    initcode: RawBytes::new(hex::decode("1122334455").unwrap()),
                                },
                            ),
                        ]
                    }

                    fn into_lotus_json(self) -> Self::LotusJson {
                        EVMConstructorParamsLotusJson {
                            creator: self.creator.0,
                            initcode: self.initcode,
                        }
                    }

                    fn from_lotus_json(lotus_json: Self::LotusJson) -> Self {
                        Self {
                            creator: fil_actor_evm_state::evm_shared::[<v $version>]::address::EthAddress(lotus_json.creator),
                            initcode: lotus_json.initcode,
                        }
                    }
                }
            }
        )+
    };
}

impl_evm_constructor_params!(10, 11, 12, 13, 14, 15, 16);
