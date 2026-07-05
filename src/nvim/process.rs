use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use rmpv::Value;

use super::rpc::{RpcClient, RpcEvent};

pub use super::rpc::RpcEvent as NvimEvent;

#[derive(Debug)]
pub struct NvimLaunchOptions {
    pub executable: PathBuf,
    pub files: Vec<PathBuf>,
    pub clean: bool,
}

pub struct NvimProcess {
    child: Child,
    rpc: RpcClient,
    events: mpsc::Receiver<RpcEvent>,
}

impl NvimProcess {
    pub fn spawn(options: NvimLaunchOptions) -> Result<Self> {
        let mut command = Command::new(&options.executable);
        apply_launch_environment(&mut command);
        if options.clean {
            command.arg("--clean");
        }
        command.arg("--embed");
        if !options.files.is_empty() {
            command.arg("--").args(&options.files);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = command.spawn().with_context(|| {
            format!("failed to start Neovim at {}", options.executable.display())
        })?;
        let stdin = child.stdin.take().context("Neovim stdin was not piped")?;
        let stdout = child.stdout.take().context("Neovim stdout was not piped")?;
        let (rpc, events) = RpcClient::start(stdin, stdout)?;

        Ok(Self { child, rpc, events })
    }

    pub fn rpc(&self) -> &RpcClient {
        &self.rpc
    }

    pub fn events(&self) -> &mpsc::Receiver<RpcEvent> {
        &self.events
    }

    pub fn attach_ui(&self, width: u64, height: u64) -> Result<()> {
        let options = Value::Map(vec![
            (Value::from("rgb"), Value::from(true)),
            (Value::from("ext_linegrid"), Value::from(true)),
        ]);
        self.rpc.request(
            "nvim_ui_attach",
            vec![Value::from(width), Value::from(height), options],
        )?;
        Ok(())
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("failed to query Neovim status")
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        self.child.wait().context("failed to wait for Neovim")
    }
}

impl Drop for NvimProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub fn discover_nvim() -> Result<PathBuf> {
    let mut searched = Vec::new();

    if let Some(path) = env::var_os("MADO_NVIM") {
        let candidate = PathBuf::from(path);
        searched.push(candidate.clone());
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }

    if let Some(path) = find_on_path(nvim_binary_name(), env::var_os("PATH").as_deref()) {
        return Ok(path);
    }

    if let Some(path) = shell_path()
        .and_then(|path| find_on_path(nvim_binary_name(), Some(path.as_os_str())))
    {
        return Ok(path);
    }

    for candidate in platform_candidates() {
        searched.push(candidate.clone());
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }

    let searched = searched
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    bail!("Neovim was not found. Set MADO_NVIM or add nvim to PATH. Checked: {searched}")
}

fn find_on_path(binary: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let path = path?;
    env::split_paths(&path)
        .map(|directory| directory.join(binary))
        .find(|candidate| is_executable_file(candidate))
}

fn apply_launch_environment(command: &mut Command) {
    #[cfg(target_os = "macos")]
    if let Some(environment) = shell_environment() {
        for (key, value) in environment {
            command.env(key, value);
        }
    }
}

#[cfg(target_os = "macos")]
fn shell_path() -> Option<&'static OsString> {
    shell_environment()?.get(OsStr::new("PATH"))
}

#[cfg(not(target_os = "macos"))]
fn shell_path() -> Option<&'static OsString> {
    None
}

#[cfg(target_os = "macos")]
fn shell_environment() -> Option<&'static HashMap<OsString, OsString>> {
    static SHELL_ENVIRONMENT: OnceLock<Option<HashMap<OsString, OsString>>> = OnceLock::new();
    SHELL_ENVIRONMENT.get_or_init(load_shell_environment).as_ref()
}

#[cfg(target_os = "macos")]
fn load_shell_environment() -> Option<HashMap<OsString, OsString>> {
    let shell = env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/zsh"));
    let output = Command::new(shell)
        .arg("-lc")
        .arg("env -0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let environment = parse_shell_environment(&output.stdout);
    if environment.is_empty() {
        None
    } else {
        Some(environment)
    }
}

fn parse_shell_environment(output: &[u8]) -> HashMap<OsString, OsString> {
    let mut environment = HashMap::new();

    for entry in output.split(|byte| *byte == 0).filter(|entry| !entry.is_empty()) {
        let Some(split_index) = entry.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = OsString::from(String::from_utf8_lossy(&entry[..split_index]).into_owned());
        if should_ignore_environment_key(&key) {
            continue;
        }
        let value =
            OsString::from(String::from_utf8_lossy(&entry[split_index + 1..]).into_owned());
        environment.insert(key, value);
    }

    environment
}

fn should_ignore_environment_key(key: &OsStr) -> bool {
    matches!(
        key.to_str(),
        Some("PWD" | "OLDPWD" | "SHLVL" | "_" | "TERM" | "TERM_PROGRAM")
    )
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn nvim_binary_name() -> &'static str {
    if cfg!(windows) { "nvim.exe" } else { "nvim" }
}

fn platform_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    #[cfg(target_os = "macos")]
    {
        candidates.extend([
            PathBuf::from("/opt/homebrew/bin/nvim"),
            PathBuf::from("/usr/local/bin/nvim"),
            PathBuf::from("/opt/local/bin/nvim"),
        ]);
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local_app_data)
                    .join("Programs")
                    .join("Neovim")
                    .join("bin")
                    .join("nvim.exe"),
            );
        }
        if let Some(program_files) = env::var_os("ProgramFiles") {
            candidates.push(
                PathBuf::from(program_files)
                    .join("Neovim")
                    .join("bin")
                    .join("nvim.exe"),
            );
        }
        if let Some(home) = env::var_os("USERPROFILE") {
            candidates.push(
                PathBuf::from(home)
                    .join("scoop")
                    .join("apps")
                    .join("neovim")
                    .join("current")
                    .join("bin")
                    .join("nvim.exe"),
            );
        }
        candidates.push(PathBuf::from(r"C:\tools\neovim\nvim-win64\bin\nvim.exe"));
    }

    candidates
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{parse_shell_environment, should_ignore_environment_key};

    #[test]
    fn parses_nul_delimited_environment_output() {
        let environment = parse_shell_environment(
            b"PATH=/opt/homebrew/bin:/usr/bin\0HOME=/Users/tester\0EMPTY=\0",
        );

        assert_eq!(
            environment.get(OsStr::new("PATH")).map(OsStr::new),
            Some(OsStr::new("/opt/homebrew/bin:/usr/bin"))
        );
        assert_eq!(
            environment.get(OsStr::new("HOME")).map(OsStr::new),
            Some(OsStr::new("/Users/tester"))
        );
        assert_eq!(
            environment.get(OsStr::new("EMPTY")).map(OsStr::new),
            Some(OsStr::new(""))
        );
    }

    #[test]
    fn ignores_shell_local_entries() {
        let environment = parse_shell_environment(b"PWD=/tmp/demo\0SHLVL=2\0_=/bin/env\0");

        assert!(environment.is_empty());
        assert!(should_ignore_environment_key(OsStr::new("PWD")));
        assert!(should_ignore_environment_key(OsStr::new("SHLVL")));
        assert!(should_ignore_environment_key(OsStr::new("_")));
        assert!(!should_ignore_environment_key(OsStr::new("PATH")));
    }
}
