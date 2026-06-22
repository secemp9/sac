use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::paths::sac_home_dir;

const MAX_SCAN_DEPTH: usize = 4;
const MAX_SCAN_DIRS: usize = 500;

mod discovery;
mod frontmatter;
mod registry;
mod template;

pub use registry::CommandRegistry;

use discovery::*;
use frontmatter::*;

#[derive(Clone, Debug)]
pub struct CommandRecord {
    pub name: String,
    pub description: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subtask: bool,
    pub template: String,
    pub source_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct CommandCatalogEntry {
    pub name: String,
    pub description: String,
}
