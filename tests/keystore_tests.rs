// Copyright 2019-2025 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

pub mod common;

use forest::{
    ENCRYPTED_KEYSTORE_NAME, FOREST_KEYSTORE_PHRASE_ENV, KEYSTORE_NAME, KeyStore, KeyStoreConfig,
};
use forest::{JWT_IDENTIFIER, verify_token};

use crate::common::{CommonArgs, create_tmp_config, daemon};

// https://github.com/ChainSafe/forest/issues/2499
#[test]
fn forest_headless_encrypt_keystore_no_passphrase_should_fail() {
    let (config_file, _data_dir) = create_tmp_config();
    daemon()
        .common_args()
        .arg("--config")
        .arg(config_file)
        .assert()
        .failure();
}

#[test]
fn forest_headless_no_encrypt_no_passphrase_should_succeed() {
    let (config_file, data_dir) = create_tmp_config();
    daemon()
        .common_args()
        .arg("--config")
        .arg(config_file)
        .arg("--encrypt-keystore")
        .arg("false")
        .assert()
        .success();

    assert!(data_dir.path().join(KEYSTORE_NAME).exists());
}

#[test]
fn forest_headless_encrypt_keystore_with_passphrase_should_succeed() {
    let (config_file, data_dir) = create_tmp_config();
    daemon()
        .env(FOREST_KEYSTORE_PHRASE_ENV, "hunter2")
        .common_args()
        .arg("--config")
        .arg(config_file)
        .assert()
        .success();

    assert!(data_dir.path().join(ENCRYPTED_KEYSTORE_NAME).exists());
}

#[test]
fn should_create_jwt_admin_token() {
    let (config_file, data_dir) = create_tmp_config();
    let token_path = data_dir.path().join("non-exsiting-dir").join("admin-token");
    daemon()
        .common_args()
        .arg("--config")
        .arg(config_file)
        .arg("--encrypt-keystore")
        .arg("false")
        .arg("--save-token")
        .arg(&token_path)
        .assert()
        .success();

    // Grab the keystore and the private key
    let keystore = KeyStore::new(KeyStoreConfig::Persistent(data_dir.path().to_owned())).unwrap();
    let key_info = keystore.get(JWT_IDENTIFIER).unwrap();
    let key = key_info.private_key();

    // Validate the token
    assert!(token_path.exists());
    let token = std::fs::read_to_string(token_path).unwrap();
    let allow = verify_token(&token, key).unwrap();
    assert!(allow.contains(&"admin".to_owned()));
}
