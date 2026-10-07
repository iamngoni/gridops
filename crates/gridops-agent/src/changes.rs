//! Reads the agent's edits back out of the sandbox.
//!
//! The checkout is committed as a baseline before the agent starts, so the
//! staged diff against it is exactly what the agent changed. Contents are read
//! from git's staged blobs, which also covers symlinks and executable bits.

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::sandbox::{ExecRequest, Sandbox, shell_quote};

/// Limits on what one fix may commit, so a runaway command cannot turn into a
/// pull request full of build output.
pub const MAX_CHANGED_FILES: usize = 60;
pub const MAX_CHANGED_BYTES: usize = 4 * 1_024 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedPath {
    pub path: String,
    pub kind: ChangeKind,
    /// Git file mode of the new version (`100644`, `100755`, `120000`).
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    pub mode: String,
    /// New contents; `None` for deletions.
    pub contents: Option<Vec<u8>>,
}

/// Turns the extracted tarball in `/workspace` into the checkout the agent
/// works on: moves it to `/workspace/repo`, makes sure git is available, and
/// commits the tree GitHub served as the diff baseline. `-f` tracks files the
/// project's own ignore rules would hide, so edits to them still show up.
pub fn prepare_checkout_script() -> &'static str {
    r#"set -e
cd /workspace
mkdir -p .gridops
source_dir=$(find . -mindepth 1 -maxdepth 1 -type d ! -name .gridops ! -name repo | head -n 1)
if [ -z "$source_dir" ]; then echo "The repository archive was empty." >&2; exit 1; fi
mv "$source_dir" repo
if ! command -v git >/dev/null 2>&1; then
  if command -v apt-get >/dev/null 2>&1; then
    (apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq git) >/dev/null
  elif command -v apk >/dev/null 2>&1; then
    apk add --no-cache git >/dev/null
  fi
fi
command -v git >/dev/null 2>&1 || { echo "git is not available in the sandbox image." >&2; exit 1; }
cd /workspace/repo
git init -q
git config user.name "GridOps"
git config user.email "gridops@localhost"
git config core.fileMode true
git add -A -f
git commit -q --no-verify --allow-empty -m baseline
git rev-parse HEAD"#
}

/// The files the agent changed since the baseline.
pub async fn changed_paths(sandbox: &dyn Sandbox) -> Result<Vec<ChangedPath>> {
    let output = sandbox
        .exec(
            ExecRequest::script(
                "git add -A && git diff --cached --raw -z --no-renames --no-abbrev",
                120,
            )
            .with_output_limit(1_024 * 1_024),
        )
        .await?;
    if output.stdout_truncated {
        bail!("The agent changed too many files to commit.");
    }
    parse_raw_diff(&output.require_success("Listing changed files")?)
}

/// Parses `git diff --raw -z`: `:old new oldsha newsha status\0path\0`.
fn parse_raw_diff(raw: &str) -> Result<Vec<ChangedPath>> {
    let mut changes = Vec::new();
    let mut fields = raw.split('\0').filter(|field| !field.is_empty());
    while let Some(meta) = fields.next() {
        let path = fields.next().context("diff entry without a path")?;
        let parts = meta.trim_start_matches(':').split(' ').collect::<Vec<_>>();
        let [_, new_mode, _, _, status] = parts.as_slice() else {
            bail!("unexpected diff entry: {meta}");
        };
        let kind = match status.chars().next() {
            Some('A') => ChangeKind::Added,
            Some('D') => ChangeKind::Deleted,
            Some('M' | 'T') => ChangeKind::Modified,
            _ => bail!("unexpected change status {status} for {path}"),
        };
        changes.push(ChangedPath {
            path: path.to_owned(),
            kind,
            mode: (*new_mode).to_owned(),
        });
    }
    Ok(changes)
}

/// Reads the contents of every changed file, enforcing the commit limits.
pub async fn collect_changes(sandbox: &dyn Sandbox) -> Result<Vec<FileChange>> {
    let paths = changed_paths(sandbox).await?;
    if paths.len() > MAX_CHANGED_FILES {
        bail!(
            "The agent changed {} files; fixes are limited to {MAX_CHANGED_FILES}.",
            paths.len()
        );
    }
    let mut total = 0;
    let mut changes = Vec::with_capacity(paths.len());
    for changed in paths {
        let contents = if changed.kind == ChangeKind::Deleted {
            None
        } else {
            let output = sandbox
                .exec(
                    ExecRequest::script(
                        format!(
                            "git cat-file blob {} | base64 -w0",
                            shell_quote(&format!(":{}", changed.path))
                        ),
                        60,
                    )
                    .with_output_limit(MAX_CHANGED_BYTES / 3 * 4 + 1_024),
                )
                .await?;
            if output.stdout_truncated {
                bail!("{} is too large to include in a fix.", changed.path);
            }
            let encoded = output.require_success(&format!("Reading {}", changed.path))?;
            let bytes = STANDARD
                .decode(encoded.trim())
                .with_context(|| format!("decode {}", changed.path))?;
            total += bytes.len();
            if total > MAX_CHANGED_BYTES {
                bail!("The agent's changes exceed the 4 MiB limit for a fix.");
            }
            Some(bytes)
        };
        changes.push(FileChange {
            path: changed.path,
            kind: changed.kind,
            mode: changed.mode,
            contents,
        });
    }
    Ok(changes)
}

/// A `git diff --stat` summary of the agent's changes, for the timeline.
pub async fn diff_stat(sandbox: &dyn Sandbox) -> Result<String> {
    sandbox
        .exec(ExecRequest::script(
            "git add -A && git diff --cached --stat",
            60,
        ))
        .await?
        .require_success("Summarising changes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_diffs_parse_into_changes() -> Result<()> {
        let raw = concat!(
            ":100644 100644 aaaa bbbb M\0src/lib.rs\0",
            ":000000 100755 0000 cccc A\0scripts/run.sh\0",
            ":100644 000000 dddd 0000 D\0old.txt\0",
        );
        let changes = parse_raw_diff(raw)?;
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].kind, ChangeKind::Modified);
        assert_eq!(changes[1].mode, "100755");
        assert_eq!(changes[1].kind, ChangeKind::Added);
        assert_eq!(changes[2].kind, ChangeKind::Deleted);
        assert!(parse_raw_diff("")?.is_empty());
        assert!(parse_raw_diff(":100644 100644 a b X\0p\0").is_err());
        Ok(())
    }

    #[test]
    fn the_prepare_script_commits_a_baseline() {
        let script = prepare_checkout_script();
        assert!(script.contains("mv \"$source_dir\" repo"));
        assert!(script.contains("git add -A -f"));
        assert!(script.contains("apt-get install -y -qq git"));
    }
}
