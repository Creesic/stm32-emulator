// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

use tempfile::TempDir;

const MAX_OUTPUT_LINES: usize = 2_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputLine {
    pub stream: OutputStream,
    pub text: String,
}

#[derive(Debug)]
pub struct TemporaryConfig {
    _directory: TempDir,
    path: PathBuf,
}

impl TemporaryConfig {
    pub fn write(yaml: &str) -> Result<Self, ProcessError> {
        let directory = tempfile::tempdir().map_err(ProcessError::io)?;
        let path = directory.path().join("resolved.yaml");
        fs::write(&path, yaml).map_err(ProcessError::io)?;
        Ok(Self {
            _directory: directory,
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn build_emulator_arguments(config_path: &Path, verbosity: u8) -> Vec<String> {
    let mut arguments = vec![config_path.to_string_lossy().into_owned()];
    arguments.extend(std::iter::repeat("-v".to_owned()).take(verbosity.into()));
    arguments
}

pub fn validate_firmware(path: &Path) -> Result<(), ProcessError> {
    if path.is_file() {
        Ok(())
    } else {
        Err(ProcessError::new(format!(
            "Firmware file does not exist: {}",
            path.display()
        )))
    }
}

pub fn discover_emulator(provided: Option<&Path>) -> Result<PathBuf, ProcessError> {
    if let Some(path) = provided {
        return validate_executable(path);
    }

    let launcher = std::env::current_exe().map_err(ProcessError::io)?;
    let sibling =
        launcher.with_file_name(format!("stm32-emulator{}", std::env::consts::EXE_SUFFIX));
    validate_executable(&sibling).map_err(|_| {
        ProcessError::new(format!(
            "Choose the stm32-emulator executable. No sibling executable was found at {}",
            sibling.display()
        ))
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    Running,
    Exited { success: bool },
}

fn classify_exit(status: std::process::ExitStatus) -> ProcessState {
    ProcessState::Exited { success: status.success() }
}

pub struct RunningEmulator {
    child: Child,
    receiver: Receiver<OutputLine>,
    readers: Vec<JoinHandle<()>>,
    output: VecDeque<OutputLine>,
}

impl RunningEmulator {
    pub fn spawn(
        executable: &Path,
        config_path: &Path,
        verbosity: u8,
    ) -> Result<Self, ProcessError> {
        validate_executable(executable)?;
        if !config_path.is_file() {
            return Err(ProcessError::new(format!(
                "Resolved configuration does not exist: {}",
                config_path.display()
            )));
        }

        let mut command = Command::new(executable);
        command
            .args(build_emulator_arguments(config_path, verbosity))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;

            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(ProcessError::io)?;
        let (sender, receiver) = mpsc::channel();
        let stdout = child.stdout.take().expect("stdout was requested as piped");
        let stderr = child.stderr.take().expect("stderr was requested as piped");

        Ok(Self {
            child,
            receiver,
            readers: vec![
                spawn_reader(stdout, OutputStream::Stdout, sender.clone()),
                spawn_reader(stderr, OutputStream::Stderr, sender),
            ],
            output: VecDeque::with_capacity(MAX_OUTPUT_LINES),
        })
    }

    pub fn poll_output(&mut self) {
        while let Ok(line) = self.receiver.try_recv() {
            if self.output.len() == MAX_OUTPUT_LINES {
                self.output.pop_front();
            }
            self.output.push_back(line);
        }
    }

    pub fn output(&self) -> &VecDeque<OutputLine> {
        &self.output
    }

    pub fn poll_state(&mut self) -> Result<ProcessState, ProcessError> {
        match self.child.try_wait().map_err(ProcessError::io)? {
            None => Ok(ProcessState::Running),
            Some(status) => Ok(classify_exit(status)),
        }
    }

    pub fn stop(&mut self) -> Result<(), ProcessError> {
        if self.child.try_wait().map_err(ProcessError::io)?.is_none() {
            self.child.kill().map_err(ProcessError::io)?;
            self.child.wait().map_err(ProcessError::io)?;
        }
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
        self.poll_output();
        Ok(())
    }
}

impl Drop for RunningEmulator {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn validate_executable(path: &Path) -> Result<PathBuf, ProcessError> {
    if path.is_file() {
        Ok(path.to_path_buf())
    } else {
        Err(ProcessError::new(format!(
            "Emulator executable does not exist: {}",
            path.display()
        )))
    }
}

fn spawn_reader<R>(
    reader: R,
    stream: OutputStream,
    sender: mpsc::Sender<OutputLine>,
) -> JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if sender.send(OutputLine { stream, text: line }).is_err() {
                break;
            }
        }
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessError(String);

impl ProcessError {
    fn new(message: String) -> Self {
        Self(message)
    }

    fn io(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for ProcessError {}

#[cfg(all(test, windows))]
mod tests {
    use std::os::windows::process::ExitStatusExt;
    use std::process::ExitStatus;

    use super::{classify_exit, ProcessState};

    #[test]
    fn a_zero_exit_code_is_classified_as_a_successful_exit() {
        assert_eq!(
            classify_exit(ExitStatus::from_raw(0)),
            ProcessState::Exited { success: true }
        );
    }

    #[test]
    fn a_nonzero_exit_code_is_classified_as_a_failed_exit() {
        assert_eq!(
            classify_exit(ExitStatus::from_raw(1)),
            ProcessState::Exited { success: false }
        );
    }
}
