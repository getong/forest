// Copyright 2019-2025 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use is_terminal::IsTerminal;

mod adaptive_value_provider;
pub use adaptive_value_provider::*;
mod logo;
pub use logo::*;
pub mod env;

#[derive(Debug, Clone, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum LoggingColor {
    Always,
    Auto,
    Never,
}

impl LoggingColor {
    pub fn coloring_enabled(&self) -> bool {
        match self {
            LoggingColor::Auto => std::io::stdout().is_terminal(),
            LoggingColor::Always => true,
            LoggingColor::Never => false,
        }
    }
}

impl Default for LoggingColor {
    fn default() -> Self {
        Self::Auto
    }
}
