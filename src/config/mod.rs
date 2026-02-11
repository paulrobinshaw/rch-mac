//! Configuration merge system
//!
//! Implements the 4-layer configuration merge:
//! 1. Built-in lane defaults
//! 2. Host/user config (~/.config/rch/xcode.toml)
//! 3. Repo config (.rch/xcode.toml)
//! 4. CLI flags

mod defaults;
mod effective;
mod merge;

pub use defaults::BuiltinDefaults;
pub use effective::{ConfigError, ConfigOrigin, ConfigSource, EffectiveConfig};
pub use merge::{deep_merge, merge_layers};
