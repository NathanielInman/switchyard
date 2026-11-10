//! Shared pieces of Switchyard: config, manifests, the snapshot model, the
//! socket protocol, error matchers and Hyprland queries.

pub mod config;
pub mod errors;
pub mod hypr;
pub mod manifest;
pub mod model;
pub mod paths;
pub mod protocol;
pub mod size;

use serde::{Deserialize, Serialize};

/// Something wrong with a config or manifest file, shown on the board.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Problem {
    pub file: String,
    pub message: String,
}
