// Copyright 2019-2025 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use std::sync::{
    Arc,
    atomic::{self, AtomicBool},
};

use crate::db::BlockstoreWithWriteBuffer;
use crate::networks::{ChainConfig, Height, NetworkChain};
use crate::shim::clock::ChainEpoch;
use crate::shim::state_tree::StateRoot;
use cid::Cid;
use fvm_ipld_blockstore::Blockstore;
use fvm_ipld_encoding::CborStore;

pub(in crate::state_migration) mod common;
mod nv17;
mod nv18;
mod nv19;
mod nv21;
mod nv21fix;
mod nv21fix2;
mod nv22;
mod nv22fix;
mod nv23;
mod nv24;
mod nv25;
mod nv26fix;
mod type_migrations;

type RunMigration<DB> = fn(&ChainConfig, &Arc<DB>, &Cid, ChainEpoch) -> anyhow::Result<Cid>;

pub fn get_migrations<DB>(chain: &NetworkChain) -> Vec<(Height, RunMigration<DB>)>
where
    DB: Blockstore + Send + Sync,
{
    match chain {
        NetworkChain::Mainnet => {
            vec![
                (Height::Shark, nv17::run_migration::<DB>),
                (Height::Hygge, nv18::run_migration::<DB>),
                (Height::Lightning, nv19::run_migration::<DB>),
                (Height::Watermelon, nv21::run_migration::<DB>),
                (Height::Dragon, nv22::run_migration::<DB>),
                (Height::Waffle, nv23::run_migration::<DB>),
                (Height::TukTuk, nv24::run_migration::<DB>),
                (Height::Teep, nv25::run_migration::<DB>),
            ]
        }
        NetworkChain::Calibnet => {
            vec![
                (Height::Shark, nv17::run_migration::<DB>),
                (Height::Hygge, nv18::run_migration::<DB>),
                (Height::Lightning, nv19::run_migration::<DB>),
                (Height::Watermelon, nv21::run_migration::<DB>),
                (Height::WatermelonFix, nv21fix::run_migration::<DB>),
                (Height::WatermelonFix2, nv21fix2::run_migration::<DB>),
                (Height::Dragon, nv22::run_migration::<DB>),
                (Height::DragonFix, nv22fix::run_migration::<DB>),
                (Height::Waffle, nv23::run_migration::<DB>),
                (Height::TukTuk, nv24::run_migration::<DB>),
                (Height::Teep, nv25::run_migration::<DB>),
                (Height::TockFix, nv26fix::run_migration::<DB>),
            ]
        }
        NetworkChain::Butterflynet => {
            vec![(Height::Teep, nv25::run_migration::<DB>)]
        }
        NetworkChain::Devnet(_) => {
            vec![
                (Height::Shark, nv17::run_migration::<DB>),
                (Height::Hygge, nv18::run_migration::<DB>),
                (Height::Lightning, nv19::run_migration::<DB>),
                (Height::Watermelon, nv21::run_migration::<DB>),
                (Height::Dragon, nv22::run_migration::<DB>),
                (Height::Waffle, nv23::run_migration::<DB>),
                (Height::TukTuk, nv24::run_migration::<DB>),
                (Height::Teep, nv25::run_migration::<DB>),
                (Height::TockFix, nv26fix::run_migration::<DB>),
            ]
        }
    }
}

/// Run state migrations
pub fn run_state_migrations<DB>(
    epoch: ChainEpoch,
    chain_config: &ChainConfig,
    db: &Arc<DB>,
    parent_state: &Cid,
) -> anyhow::Result<Option<Cid>>
where
    DB: Blockstore + Send + Sync,
{
    // ~10MB RAM per 10k buffer
    let db_write_buffer = match std::env::var("FOREST_STATE_MIGRATION_DB_WRITE_BUFFER") {
        Ok(v) => v.parse().ok(),
        _ => None,
    }
    .unwrap_or(10000);
    let mappings = get_migrations(&chain_config.network);

    // Make sure bundle is defined.
    static BUNDLE_CHECKED: AtomicBool = AtomicBool::new(false);
    if !BUNDLE_CHECKED.load(atomic::Ordering::Relaxed) {
        BUNDLE_CHECKED.store(true, atomic::Ordering::Relaxed);
        for (info_height, info) in chain_config.height_infos.iter() {
            for (height, _) in &mappings {
                if height == info_height {
                    assert!(
                        info.bundle.is_some(),
                        "Actor bundle info for height {height} needs to be defined in `src/networks/mod.rs` to run state migration"
                    );
                    break;
                }
            }
        }
    }

    for (height, migrate) in mappings {
        if epoch == chain_config.epoch(height) {
            tracing::info!("Running {height} migration at epoch {epoch}");
            let start_time = std::time::Instant::now();
            let db = Arc::new(BlockstoreWithWriteBuffer::new_with_capacity(
                db.clone(),
                db_write_buffer,
            ));
            let new_state = migrate(chain_config, &db, parent_state, epoch)?;
            let elapsed = start_time.elapsed();
            // `new_state_actors` is the Go state migration output, log for comparision
            let new_state_actors = db
                .get_cbor::<StateRoot>(&new_state)
                .ok()
                .flatten()
                .map(|sr| format!("{}", sr.actors))
                .unwrap_or_default();
            if new_state != *parent_state {
                crate::utils::misc::reveal_upgrade_logo(height.into());
                tracing::info!(
                    "State migration at height {height}(epoch {epoch}) was successful, Previous state: {parent_state}, new state: {new_state}, new state actors: {new_state_actors}. Took: {elapsed}.",
                    elapsed = humantime::format_duration(elapsed)
                );
            } else {
                anyhow::bail!(
                    "State post migration at height {height} must not match. Previous state: {parent_state}, new state: {new_state}, new state actors: {new_state_actors}. Took {elapsed}.",
                    elapsed = humantime::format_duration(elapsed)
                );
            }

            return Ok(Some(new_state));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests;
