//! Per-profile directory layout. Profiles are isolated: their world, mods,
//! configs and logs never leak into another profile. Jars and assets stay in
//! the shared cache and are referenced in place, so nothing is duplicated.

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::paths;

#[derive(Clone, Debug)]
pub struct InstancePaths {
    pub id: String,
    pub root: PathBuf,
    pub game_dir: PathBuf,
    pub mods: PathBuf,
    pub resourcepacks: PathBuf,
    pub shaderpacks: PathBuf,
    pub saves: PathBuf,
    pub config: PathBuf,
    pub logs: PathBuf,
    pub natives: PathBuf,
}

impl InstancePaths {
    pub fn new(id: &str) -> Self {
        let root = paths::instance_dir(id);
        let game_dir = root.join("game");
        Self {
            id: id.to_string(),
            mods: game_dir.join("mods"),
            resourcepacks: game_dir.join("resourcepacks"),
            shaderpacks: game_dir.join("shaderpacks"),
            saves: game_dir.join("saves"),
            config: game_dir.join("config"),
            logs: root.join("logs"),
            natives: root.join("natives"),
            game_dir,
            root,
        }
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.root,
            &self.game_dir,
            &self.mods,
            &self.resourcepacks,
            &self.shaderpacks,
            &self.saves,
            &self.config,
            &self.logs,
            &self.natives,
        ] {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        Ok(())
    }
}
