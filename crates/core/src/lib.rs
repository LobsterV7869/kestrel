//! Kestrel core: all launcher logic that is independent of the UI shell.
//!
//! Module layout mirrors the launcher's responsibilities so it can be reused
//! by the Tauri app, the CLI and tests:
//!
//! - [`auth`]      Microsoft / Xbox Live / Minecraft token chain (stage 3)
//! - [`versions`]  Mojang version manifest + version JSON rules (stage 2)
//! - [`java`]      JVM detection and runtime provisioning (stage 4)
//! - [`loaders`]   Fabric / Quilt / NeoForge / Forge installers (stage 5)
//! - [`instances`] Profiles, isolated game dirs, import/export (stage 8)
//! - [`content`]   Modrinth/CurseForge browser and installs (stage 5)
//! - [`launch`]    Command-line construction and process supervision (stage 2)
//! - [`servers`]   Server address parsing, SRV and status ping (stage 7)
//! - [`bench`]     Timing harness used by `kestrel bench`

pub mod bench;
pub mod cli;
pub mod config;
pub mod error;
pub mod http;
pub mod logging;
pub mod paths;
pub mod state;

pub use error::{Error, Result};

pub const APP_NAME: &str = "Kestrel";
pub const LEGAL_NOTICE: &str =
    "Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft.";
