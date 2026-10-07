//! The isolated container the agent works in, as seen by the tools.
//!
//! `GridOps`' runner manager implements this over Docker. The container holds a
//! copy of the repository at `/workspace/repo` and nothing secret: tools only
//! ever pass it file contents and shell commands.

use std::collections::BTreeMap;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;

/// Where the repository lives inside the sandbox.
pub const REPO_ROOT: &str = "/workspace/repo";
/// Scratch space for uploads; never part of the repository.
pub const SCRATCH_ROOT: &str = "/workspace/.gridops";

#[derive(Debug, Clone)]
pub struct ExecRequest {
    pub command: Vec<String>,
    pub workdir: String,
    pub timeout_seconds: u64,
    pub max_output_bytes: usize,
    pub env: BTreeMap<String, String>,
}

impl ExecRequest {
    /// A `bash -lc` script run from the repository root.
    pub fn script(script: impl Into<String>, timeout_seconds: u64) -> Self {
        Self {
            command: vec!["bash".into(), "-lc".into(), script.into()],
            workdir: REPO_ROOT.into(),
            timeout_seconds,
            max_output_bytes: 64 * 1_024,
            env: BTreeMap::from([
                ("CI".to_owned(), "true".to_owned()),
                ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
            ]),
        }
    }

    #[must_use]
    pub fn with_output_limit(mut self, bytes: usize) -> Self {
        self.max_output_bytes = bytes;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExecOutput {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
    pub duration_ms: u64,
}

impl ExecOutput {
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }

    /// The stdout of a command that must succeed, or its stderr as the error.
    pub fn require_success(self, what: &str) -> Result<String> {
        if self.success() {
            return Ok(self.stdout);
        }
        let detail = if self.stderr.trim().is_empty() {
            self.stdout
        } else {
            self.stderr
        };
        bail!(
            "{what} failed (exit {}{}): {}",
            self.exit_code,
            if self.timed_out { ", timed out" } else { "" },
            detail.trim().chars().take(800).collect::<String>()
        )
    }
}

#[async_trait]
pub trait Sandbox: Send + Sync {
    async fn exec(&self, request: ExecRequest) -> Result<ExecOutput>;

    /// Extracts a tar archive at `destination`, an absolute directory.
    async fn upload_tar(&self, destination: &str, archive: Vec<u8>) -> Result<()>;
}

/// Quotes a value for a POSIX shell.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Checks a model-supplied path and returns it relative to the repository
/// root. Absolute paths, parent references and the `.git` directory are
/// refused so the agent stays inside the checkout it was given.
pub fn repo_relative_path(path: &str) -> Result<String> {
    let trimmed = path.trim();
    let without_prefix = trimmed
        .strip_prefix(REPO_ROOT)
        .map(|rest| rest.trim_start_matches('/'))
        .unwrap_or(trimmed)
        .trim_start_matches("./");
    if without_prefix.is_empty() || without_prefix == "." {
        return Ok(".".into());
    }
    if without_prefix.starts_with('/') {
        bail!("Use a path relative to the repository root, not {trimmed}.");
    }
    let mut parts = Vec::new();
    for part in without_prefix.split('/') {
        match part {
            "" | "." => {}
            ".." => bail!("Paths may not leave the repository: {trimmed}"),
            ".git" => bail!("The .git directory is managed by GridOps."),
            part if part.contains('\0') => bail!("Invalid path."),
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Ok(".".into());
    }
    Ok(parts.join("/"))
}

/// A one-file tar archive, used to move file contents into the sandbox
/// without putting them on a command line.
pub fn single_file_tar(name: &str, contents: &[u8]) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(u64::try_from(contents.len()).context("file is too large")?);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, name, contents)
        .context("build upload archive")?;
    builder.into_inner().context("finish upload archive")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_repository() -> Result<()> {
        assert_eq!(repo_relative_path("src/lib.rs")?, "src/lib.rs");
        assert_eq!(repo_relative_path("./src//lib.rs")?, "src/lib.rs");
        assert_eq!(repo_relative_path("/workspace/repo/a/b")?, "a/b");
        assert_eq!(repo_relative_path("")?, ".");
        assert!(repo_relative_path("../etc/passwd").is_err());
        assert!(repo_relative_path("/etc/passwd").is_err());
        assert!(repo_relative_path("a/../../b").is_err());
        assert!(repo_relative_path(".git/config").is_err());
        Ok(())
    }

    #[test]
    fn quoting_survives_single_quotes() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("plain"), "'plain'");
    }

    #[test]
    fn single_file_archives_hold_the_contents() -> Result<()> {
        let archive = single_file_tar("upload/file.txt", b"hello")?;
        let mut reader = tar::Archive::new(archive.as_slice());
        let mut entries = reader.entries()?;
        let mut entry = entries.next().context("one entry")??;
        assert_eq!(entry.path()?.to_string_lossy(), "upload/file.txt");
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut entry, &mut contents)?;
        assert_eq!(contents, "hello");
        Ok(())
    }

    #[test]
    fn failed_commands_report_stderr() {
        let output = ExecOutput {
            exit_code: 2,
            stderr: "boom".into(),
            ..ExecOutput::default()
        };
        let error = output
            .require_success("git status")
            .err()
            .map(|error| error.to_string());
        assert_eq!(error.as_deref(), Some("git status failed (exit 2): boom"));
    }
}
