use std::fmt;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use crate::launcher::LauncherCpuModel;

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct WindowPlacement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct SavedLauncherState {
    pub firmware: Option<PathBuf>,
    pub svd: Option<PathBuf>,
    pub emulator_executable: Option<PathBuf>,
    pub selected_variant: Option<String>,
    pub filter: String,
    pub manual_enabled: bool,
    pub manual_cpu_model: LauncherCpuModel,
    pub manual_svd: String,
    pub manual_vector_table: String,
    pub manual_flash_start: String,
    pub manual_flash_size: String,
    pub manual_ram_start: String,
    pub manual_ram_size: String,
    // #[serde(default)] so workspace.yaml files saved before this field
    // existed still deserialize (a missing required field would otherwise
    // fail the whole struct and silently reset all saved state, per
    // WorkspaceStore::load()'s unwrap_or_default() fallback).
    #[serde(default)]
    pub usb_cdc_tcp_port: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct Workspace {
    pub window: Option<WindowPlacement>,
    pub state: SavedLauncherState,
}

pub struct WorkspaceStore {
    directory: PathBuf,
}

impl WorkspaceStore {
    pub fn for_current_user() -> Result<Self, WorkspaceError> {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_STATE_HOME").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state")))
            .ok_or_else(|| WorkspaceError("No user state directory is available.".to_owned()))?;
        Self::in_directory(root.join("stm32-emulator").join("launcher"))
    }
    pub fn in_directory(directory: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let directory = directory.into();
        fs::create_dir_all(&directory).map_err(WorkspaceError::io)?;
        Ok(Self { directory })
    }

    pub fn workspace_path(&self) -> PathBuf {
        self.directory.join("workspace.yaml")
    }

    pub fn imgui_ini_path(&self) -> PathBuf {
        self.directory.join("imgui.ini")
    }

    pub fn load(&self) -> Result<Workspace, WorkspaceError> {
        match fs::read_to_string(self.workspace_path()) {
            Ok(contents) => Ok(serde_yaml::from_str(&contents).unwrap_or_default()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Workspace::default()),
            Err(error) => Err(WorkspaceError::io(error)),
        }
    }

    pub fn save(&self, workspace: &Workspace) -> Result<(), WorkspaceError> {
        let yaml = serde_yaml::to_string(workspace).map_err(WorkspaceError::yaml)?;
        fs::write(self.workspace_path(), yaml).map_err(WorkspaceError::io)
    }
}

#[derive(Debug)]
pub struct WorkspaceError(String);

impl WorkspaceError {
    fn io(error: std::io::Error) -> Self { Self(error.to_string()) }
    fn yaml(error: serde_yaml::Error) -> Self { Self(error.to_string()) }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for WorkspaceError {}
