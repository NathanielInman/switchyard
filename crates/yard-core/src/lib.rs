//! Shared pieces of Switchyard: config, manifests, the snapshot model, the
//! socket protocol, error matchers, search ranking and Hyprland queries.

pub mod config;
pub mod errors;
pub mod history;
pub mod hypr;
pub mod inspect;
pub mod manifest;
pub mod model;
pub mod paths;
pub mod protocol;
pub mod search;
pub mod sibling;
pub mod size;
pub mod transport;

use serde::{Deserialize, Serialize};

/// Something wrong with a config or manifest file, shown on the board.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Problem {
    pub file: String,
    pub message: String,
}
