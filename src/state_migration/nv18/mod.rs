// Copyright 2019-2025 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

//! This module contains the migration logic for the `NV18` upgrade.
//! The corresponding Go implementation can be found here:
//! <https://github.com/filecoin-project/go-state-types/blob/master/builtin/v10/migration/top.go>

mod eam;
mod eth_account;
mod init;
mod migration;

/// Run migration for `NV18`. This should be the only exported method in this
/// module.
pub use migration::run_migration;

use crate::{define_system_states, impl_system, impl_verifier};

define_system_states!(
    fil_actor_system_state::v9::State,
    fil_actor_system_state::v10::State
);

impl_system!();
impl_verifier!();
