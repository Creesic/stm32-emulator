// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct LauncherState {
    pub firmware: Option<PathBuf>,
    pub svd: Option<PathBuf>,
    pub emulator_executable: Option<PathBuf>,
    pub selected_variant: Option<String>,
    pub running: bool,
    pub last_error: Option<String>,
}

impl LauncherState {
    pub fn can_run(&self) -> bool {
        self.firmware.is_some() && self.selected_variant.is_some() && !self.running
    }
}
