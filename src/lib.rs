//! fdt: el CLI de Foundathyon. Un cliente delgado sobre la API de Core (detrás
//! del api-gateway) pensado para personas y para agentes de IA: `--json` en
//! todo, códigos de salida estables, nunca pregunta sin terminal y los
//! secretos no salen salvo que se pidan.

pub mod api;
pub mod cli;
pub mod commands;
pub mod config;
pub mod context;
pub mod envfile;
pub mod failure;
pub mod output;
pub mod resolve;
pub mod secrets;
pub mod skills;
pub mod ui;
pub mod update;

/// The version this binary reports, set by the release workflow from the tag.
pub const VERSION: &str = match option_env!("FDT_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// Public gateway in front of Core: `https://api.foundathyon.com` with the
/// `/core` prefix that the gateway maps to Core's ROOT_PATH.
pub const DEFAULT_API_URL: &str = "https://api.foundathyon.com/core/api/v1";
