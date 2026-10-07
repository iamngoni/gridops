//! The tools the fix agent calls, all executed inside the sandbox.
//!
//! Every failure is returned to the model as text rather than raised, so a bad
//! path or a failing command becomes something the model can react to instead
//! of ending the run.

use std::fmt::Write as _;

use agent_runtime::{ToolCall, ToolDefinition};
use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::sandbox::{
    ExecOutput, ExecRequest, REPO_ROOT, SCRATCH_ROOT, Sandbox, repo_relative_path, shell_quote,
    single_file_tar,
};

pub const FINISH_TOOL: &str = "finish";

const READ_DEFAULT_LINES: usize = 400;
const READ_MAX_LINE_CHARS: usize = 500;
const LIST_MAX_ENTRIES: usize = 400;
const SEARCH_MAX_LINES: usize = 300;
const COMMAND_DEFAULT_TIMEOUT: u64 = 300;
const COMMAND_MAX_TIMEOUT: u64 = 1_200;
const COMMAND_OUTPUT_CHARS: usize = 16_000;
const WRITE_MAX_BYTES: usize = 1_024 * 1_024;
const EDIT_MAX_BYTES: usize = 2 * 1_024 * 1_024;

/// What one tool call produced: the text the model reads next, and a short
/// line for the run's timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub text: String,
    pub title: String,
    pub detail: Option<String>,
    pub is_error: bool,
}

impl ToolResult {
    fn ok(title: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            title: title.into(),
            detail: Some(clip(&text, 4_000)),
            text,
            is_error: false,
        }
    }

    fn error(title: impl Into<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            title: title.into(),
            detail: Some(message.clone()),
            text: format!("Error: {message}"),
            is_error: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishOutcome {
    PullRequest,
    Diagnosis,
}

/// The agent's final report, from the `finish` tool.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FinishRequest {
    pub outcome: FinishOutcome,
    pub title: String,
    pub summary: String,
    pub body: String,
    #[serde(default)]
    pub verification: Option<String>,
}

impl FinishRequest {
    pub fn parse(arguments: &Value) -> Result<Self> {
        let request: Self = serde_json::from_value(arguments.clone())
            .context("finish needs outcome, title, summary and body")?;
        if request.title.trim().is_empty() || request.summary.trim().is_empty() {
            bail!("finish needs a non-empty title and summary");
        }
        if request.title.chars().count() > 120 {
            bail!("Keep the title under 120 characters");
        }
        Ok(request)
    }
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        definition(
            "list_files",
            "List files in the repository (respecting .gitignore). Use it to find your way around before reading.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory relative to the repository root. Defaults to the root." },
                    "max_depth": { "type": "integer", "minimum": 1, "maximum": 8, "description": "How many directory levels to show. Defaults to 3." }
                },
                "additionalProperties": false
            }),
        ),
        definition(
            "read_file",
            "Read a text file with line numbers. Reads up to 400 lines per call; pass start_line/end_line for other parts of long files.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "start_line": { "type": "integer", "minimum": 1 },
                    "end_line": { "type": "integer", "minimum": 1 }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        definition(
            "search",
            "Search file contents with git grep. Returns matching lines as path:line:text.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Extended regular expression, or a literal string when fixed_string is true." },
                    "path": { "type": "string", "description": "Limit the search to this directory or file." },
                    "glob": { "type": "string", "description": "Limit the search to files matching this glob, e.g. '*.ts'." },
                    "fixed_string": { "type": "boolean" },
                    "case_sensitive": { "type": "boolean", "description": "Defaults to true." }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        definition(
            "edit_file",
            "Replace an exact piece of text in a file. old_text must match exactly once (including whitespace) unless replace_all is true. Prefer this over write_file for changes to existing files.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_text": { "type": "string" },
                    "new_text": { "type": "string" },
                    "replace_all": { "type": "boolean" }
                },
                "required": ["path", "old_text", "new_text"],
                "additionalProperties": false
            }),
        ),
        definition(
            "write_file",
            "Create a file, or replace a file's entire contents. Parent directories are created as needed.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        ),
        definition(
            "run_command",
            "Run a bash command from the repository root, e.g. to install dependencies, build, or run the failing tests. Output is truncated in the middle when long. The sandbox has network access but no credentials.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout_seconds": { "type": "integer", "minimum": 5, "maximum": COMMAND_MAX_TIMEOUT, "description": "Defaults to 300." }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        ),
        definition(
            FINISH_TOOL,
            "End the run. Use outcome pull_request when you changed files that fix the failure; GridOps commits your changes and opens a draft pull request using title and body. Use outcome diagnosis when the failure is not fixable in this repository (runner resources, infrastructure, secrets, flaky external services) or you could not find a safe fix; body then explains the cause and what to change.",
            json!({
                "type": "object",
                "properties": {
                    "outcome": { "type": "string", "enum": ["pull_request", "diagnosis"] },
                    "title": { "type": "string", "description": "Pull request title, or a one-line diagnosis. Under 120 characters." },
                    "summary": { "type": "string", "description": "One or two sentences on the cause and the fix." },
                    "body": { "type": "string", "description": "Markdown: root cause, what changed or what to change, and evidence from the logs." },
                    "verification": { "type": "string", "description": "What you ran to check the fix, and the result. Say so plainly if you could not verify it." }
                },
                "required": ["outcome", "title", "summary", "body"],
                "additionalProperties": false
            }),
        ),
    ]
}

fn definition(name: &str, description: &str, schema: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema: schema,
    }
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max_depth: Option<usize>,
}

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    end_line: Option<usize>,
}

#[derive(Deserialize)]
struct SearchArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    fixed_string: bool,
    #[serde(default)]
    case_sensitive: Option<bool>,
}

#[derive(Deserialize)]
struct EditArgs {
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct CommandArgs {
    command: String,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

/// Runs one non-finish tool call. Never fails: errors come back as text.
pub async fn execute(sandbox: &dyn Sandbox, call: &ToolCall) -> ToolResult {
    let arguments = &call.arguments;
    let result = match call.name.as_str() {
        "list_files" => match parse::<ListArgs>(arguments) {
            Ok(args) => list_files(sandbox, args).await,
            Err(error) => Err(error),
        },
        "read_file" => match parse::<ReadArgs>(arguments) {
            Ok(args) => read_file(sandbox, args).await,
            Err(error) => Err(error),
        },
        "search" => match parse::<SearchArgs>(arguments) {
            Ok(args) => search(sandbox, args).await,
            Err(error) => Err(error),
        },
        "edit_file" => match parse::<EditArgs>(arguments) {
            Ok(args) => edit_file(sandbox, args).await,
            Err(error) => Err(error),
        },
        "write_file" => match parse::<WriteArgs>(arguments) {
            Ok(args) => write_file_tool(sandbox, args).await,
            Err(error) => Err(error),
        },
        "run_command" => match parse::<CommandArgs>(arguments) {
            Ok(args) => run_command(sandbox, args).await,
            Err(error) => Err(error),
        },
        other => Err(anyhow::anyhow!(
            "Unknown tool {other}. Use one of the tools you were given."
        )),
    };
    result.unwrap_or_else(|error| ToolResult::error(tool_title(call), error.to_string()))
}

fn parse<T: serde::de::DeserializeOwned>(arguments: &Value) -> Result<T> {
    serde_json::from_value(arguments.clone()).context("Invalid arguments for this tool")
}

fn tool_title(call: &ToolCall) -> String {
    let path = call
        .arguments
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match call.name.as_str() {
        "list_files" => format!("List {}", if path.is_empty() { "." } else { path }),
        "read_file" => format!("Read {path}"),
        "search" => format!(
            "Search {}",
            call.arguments
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
        "edit_file" => format!("Edit {path}"),
        "write_file" => format!("Write {path}"),
        "run_command" => format!(
            "Run {}",
            clip(
                call.arguments
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                80
            )
        ),
        other => other.to_owned(),
    }
}

async fn list_files(sandbox: &dyn Sandbox, args: ListArgs) -> Result<ToolResult> {
    let path = repo_relative_path(args.path.as_deref().unwrap_or("."))?;
    let depth = args.max_depth.unwrap_or(3).clamp(1, 8);
    let output = sandbox
        .exec(
            ExecRequest::script(
                format!(
                    "git ls-files --cached --others --exclude-standard -- {}",
                    shell_quote(&path)
                ),
                60,
            )
            .with_output_limit(1_024 * 1_024),
        )
        .await?
        .require_success("Listing files")?;
    let listing = summarize_listing(&path, output.lines(), depth);
    Ok(ToolResult::ok(format!("List {path}"), listing))
}

/// Shows files down to `depth` levels below `root` and folds deeper files
/// into a per-directory count, so large trees stay readable.
fn summarize_listing<'a>(root: &str, files: impl Iterator<Item = &'a str>, depth: usize) -> String {
    let prefix = if root == "." {
        String::new()
    } else {
        format!("{}/", root.trim_end_matches('/'))
    };
    let mut shown = Vec::new();
    let mut folded: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut total = 0;
    for file in files {
        let relative = if prefix.is_empty() {
            file
        } else if let Some(relative) = file.strip_prefix(&prefix) {
            relative
        } else if file == root {
            // The path named a file rather than a directory.
            total += 1;
            shown.push(file.to_owned());
            continue;
        } else {
            continue;
        };
        total += 1;
        let parts = relative.split('/').collect::<Vec<_>>();
        if parts.len() <= depth {
            shown.push(format!("{prefix}{relative}"));
        } else {
            let directory = parts[..depth].join("/");
            *folded.entry(format!("{prefix}{directory}/")).or_default() += 1;
        }
    }
    if total == 0 {
        return format!("No files under {root}.");
    }
    let mut lines = shown;
    lines.extend(
        folded
            .into_iter()
            .map(|(directory, count)| format!("{directory} ({count} more files)")),
    );
    lines.sort();
    let omitted = lines.len().saturating_sub(LIST_MAX_ENTRIES);
    lines.truncate(LIST_MAX_ENTRIES);
    let mut text = lines.join("\n");
    if omitted > 0 {
        let _ = write!(
            text,
            "\n… {omitted} more entries. List a subdirectory to see them."
        );
    }
    text
}

async fn read_file(sandbox: &dyn Sandbox, args: ReadArgs) -> Result<ToolResult> {
    let path = repo_relative_path(&args.path)?;
    let start = args.start_line.unwrap_or(1).max(1);
    let end = args
        .end_line
        .unwrap_or(start + READ_DEFAULT_LINES - 1)
        .clamp(start, start + READ_DEFAULT_LINES * 2 - 1);
    let quoted = shell_quote(&path);
    let script = format!(
        r#"f={quoted}
if [ -d "$f" ]; then echo "__GRIDOPS_DIRECTORY__"; exit 0; fi
if [ ! -f "$f" ]; then echo "__GRIDOPS_MISSING__"; exit 0; fi
if [ -s "$f" ] && ! grep -Iq '' "$f"; then echo "__GRIDOPS_BINARY__"; exit 0; fi
echo "__GRIDOPS_TOTAL__ $(wc -l < "$f")"
sed -n '{start},{end}p' "$f""#
    );
    let output = sandbox
        .exec(ExecRequest::script(script, 60).with_output_limit(512 * 1_024))
        .await?
        .require_success("Reading the file")?;
    let mut lines = output.lines();
    let first = lines.next().unwrap_or_default();
    let title = format!("Read {path}");
    match first {
        "__GRIDOPS_DIRECTORY__" => bail!("{path} is a directory. Use list_files."),
        "__GRIDOPS_MISSING__" => bail!("{path} does not exist."),
        "__GRIDOPS_BINARY__" => bail!("{path} is a binary file."),
        _ => {}
    }
    let total = first
        .strip_prefix("__GRIDOPS_TOTAL__ ")
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut text = String::new();
    let mut last = start.saturating_sub(1);
    for (offset, line) in lines.enumerate() {
        last = start + offset;
        let _ = writeln!(text, "{last:>6}\t{}", clip(line, READ_MAX_LINE_CHARS));
    }
    if text.is_empty() {
        return Ok(ToolResult::ok(
            title,
            format!("{path} has {total} lines; nothing at line {start}."),
        ));
    }
    let header = format!("{path} (lines {start}-{last} of {total})\n");
    let footer = if last < total {
        format!(
            "… {} more lines. Read from line {} to continue.",
            total - last,
            last + 1
        )
    } else {
        String::new()
    };
    let result = ToolResult {
        title,
        detail: Some(format!("Lines {start}-{last} of {total}")),
        text: format!("{header}{text}{footer}"),
        is_error: false,
    };
    Ok(result)
}

async fn search(sandbox: &dyn Sandbox, args: SearchArgs) -> Result<ToolResult> {
    if args.pattern.is_empty() {
        bail!("pattern is empty");
    }
    let path = repo_relative_path(args.path.as_deref().unwrap_or("."))?;
    let mut command = String::from("git grep -n -I --untracked --no-color --full-name");
    if args.case_sensitive == Some(false) {
        command.push_str(" -i");
    }
    command.push_str(if args.fixed_string { " -F" } else { " -E" });
    let _ = write!(
        command,
        " -e {} -- {}",
        shell_quote(&args.pattern),
        shell_quote(&path)
    );
    if let Some(glob) = args.glob.as_deref().filter(|glob| !glob.is_empty()) {
        let scoped = if path == "." {
            glob.to_owned()
        } else {
            format!("{path}/**/{glob}")
        };
        let _ = write!(command, " {}", shell_quote(&format!(":(glob){scoped}")));
    }
    let _ = write!(command, " | head -n {}", SEARCH_MAX_LINES + 1);
    let output = sandbox
        .exec(ExecRequest::script(
            format!("set -o pipefail; {command}"),
            120,
        ))
        .await?;
    let title = format!("Search {}", clip(&args.pattern, 60));
    // git grep exits 1 with no output when nothing matches.
    if output.exit_code == 1 && output.stdout.trim().is_empty() && output.stderr.trim().is_empty() {
        return Ok(ToolResult::ok(title, "No matches."));
    }
    // head closing the pipe early makes git grep exit 141; the output is fine.
    if !output.success() && output.exit_code != 141 {
        bail!("Search failed: {}", clip(output.stderr.trim(), 600));
    }
    let mut lines = output
        .stdout
        .lines()
        .map(|line| clip(line, READ_MAX_LINE_CHARS))
        .collect::<Vec<_>>();
    let more = lines.len() > SEARCH_MAX_LINES;
    lines.truncate(SEARCH_MAX_LINES);
    let count = lines.len();
    let mut text = lines.join("\n");
    if more {
        text.push_str("\n… more matches. Narrow the pattern or path.");
    }
    Ok(ToolResult {
        detail: Some(format!(
            "{count}{} matching lines",
            if more { "+" } else { "" }
        )),
        title,
        text,
        is_error: false,
    })
}

/// Reads a whole text file for editing.
async fn read_whole_file(sandbox: &dyn Sandbox, path: &str) -> Result<Option<String>> {
    let quoted = shell_quote(path);
    let output = sandbox
        .exec(
            ExecRequest::script(
                format!(r#"if [ -f {quoted} ]; then cat -- {quoted}; else exit 44; fi"#),
                60,
            )
            .with_output_limit(EDIT_MAX_BYTES + 1_024),
        )
        .await?;
    if output.exit_code == 44 {
        return Ok(None);
    }
    if output.stdout_truncated {
        bail!("{path} is too large to edit here; use run_command with a targeted tool instead.");
    }
    let contents = output.require_success("Reading the file")?;
    if contents.contains('\u{fffd}') {
        bail!("{path} is not valid UTF-8; edit it with run_command instead.");
    }
    Ok(Some(contents))
}

/// Writes contents through an uploaded scratch file. Writing with `cat >`
/// keeps an existing file's mode, so executable scripts stay executable.
pub async fn write_file(sandbox: &dyn Sandbox, path: &str, contents: &str) -> Result<()> {
    if contents.len() > WRITE_MAX_BYTES {
        bail!("Files written by the agent are limited to 1 MiB.");
    }
    let name = format!("upload-{}", uuid::Uuid::new_v4());
    let archive = single_file_tar(&name, contents.as_bytes())?;
    sandbox.upload_tar(SCRATCH_ROOT, archive).await?;
    let source = shell_quote(&format!("{SCRATCH_ROOT}/{name}"));
    let target = shell_quote(path);
    sandbox
        .exec(ExecRequest::script(
            format!(
                r#"mkdir -p "$(dirname -- {target})" && cat -- {source} > {target}; status=$?; rm -f -- {source}; exit $status"#
            ),
            60,
        ))
        .await?
        .require_success("Writing the file")?;
    Ok(())
}

async fn edit_file(sandbox: &dyn Sandbox, args: EditArgs) -> Result<ToolResult> {
    let path = repo_relative_path(&args.path)?;
    if args.old_text.is_empty() {
        bail!("old_text is empty. Use write_file to create a file.");
    }
    let Some(contents) = read_whole_file(sandbox, &path).await? else {
        bail!("{path} does not exist. Use write_file to create it.");
    };
    let count = contents.matches(&args.old_text).count();
    if count == 0 {
        bail!(
            "old_text was not found in {path}. Read the file again and copy the text exactly, including indentation."
        );
    }
    if count > 1 && !args.replace_all {
        bail!(
            "old_text appears {count} times in {path}. Include more surrounding lines so it matches once, or set replace_all."
        );
    }
    let updated = if args.replace_all {
        contents.replace(&args.old_text, &args.new_text)
    } else {
        contents.replacen(&args.old_text, &args.new_text, 1)
    };
    write_file(sandbox, &path, &updated).await?;
    let replaced = if args.replace_all { count } else { 1 };
    Ok(ToolResult {
        title: format!("Edit {path}"),
        detail: Some(diff_preview(&args.old_text, &args.new_text)),
        text: format!(
            "Replaced {replaced} occurrence{} in {path}.",
            if replaced == 1 { "" } else { "s" }
        ),
        is_error: false,
    })
}

async fn write_file_tool(sandbox: &dyn Sandbox, args: WriteArgs) -> Result<ToolResult> {
    let path = repo_relative_path(&args.path)?;
    if path == "." {
        bail!("Give a file path.");
    }
    write_file(sandbox, &path, &args.content).await?;
    let lines = args.content.lines().count();
    Ok(ToolResult {
        title: format!("Write {path}"),
        detail: Some(clip(&args.content, 2_000)),
        text: format!("Wrote {lines} lines to {path}."),
        is_error: false,
    })
}

async fn run_command(sandbox: &dyn Sandbox, args: CommandArgs) -> Result<ToolResult> {
    let command = args.command.trim();
    if command.is_empty() {
        bail!("command is empty");
    }
    let timeout = args
        .timeout_seconds
        .unwrap_or(COMMAND_DEFAULT_TIMEOUT)
        .clamp(5, COMMAND_MAX_TIMEOUT);
    let output = sandbox
        .exec(ExecRequest::script(command, timeout).with_output_limit(96 * 1_024))
        .await?;
    let title = format!(
        "Run {} → {}",
        clip(command, 80),
        if output.timed_out {
            "timed out".to_owned()
        } else {
            format!("exit {}", output.exit_code)
        }
    );
    let text = format_command_output(command, &output);
    Ok(ToolResult {
        detail: Some(clip_middle(
            &[output.stdout.trim_end(), output.stderr.trim_end()]
                .iter()
                .filter(|part| !part.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join("\n"),
            4_000,
        )),
        title,
        text,
        is_error: false,
    })
}

fn format_command_output(command: &str, output: &ExecOutput) -> String {
    let mut text = format!(
        "$ {command}\nexit code: {}{} ({:.1}s)\n",
        output.exit_code,
        if output.timed_out { ", timed out" } else { "" },
        output.duration_ms as f64 / 1_000.0
    );
    for (label, stream) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        if stream.trim().is_empty() {
            continue;
        }
        let _ = write!(
            text,
            "--- {label} ---\n{}\n",
            clip_middle(stream.trim_end(), COMMAND_OUTPUT_CHARS)
        );
    }
    text
}

fn diff_preview(old: &str, new: &str) -> String {
    let mut preview = String::new();
    for line in old.lines().take(20) {
        let _ = writeln!(preview, "- {line}");
    }
    for line in new.lines().take(20) {
        let _ = writeln!(preview, "+ {line}");
    }
    clip(&preview, 2_000)
}

/// Keeps the start of `value`, for single lines and previews.
pub fn clip(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut clipped = value.chars().take(max_chars).collect::<String>();
    clipped.push('…');
    clipped
}

/// Keeps the start and the end of `value`. Build and test output carries the
/// failure at the end, so the end gets the larger share.
pub fn clip_middle(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    let head = max_chars / 4;
    let tail = max_chars - head;
    let start = value.chars().take(head).collect::<String>();
    let end = value.chars().skip(count - tail).collect::<String>();
    format!(
        "{start}\n… [{} characters omitted] …\n{end}",
        count - head - tail
    )
}

/// The repository root, for prompts.
pub fn repo_root() -> &'static str {
    REPO_ROOT
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;

    /// A sandbox that answers each exec from a script of canned outputs and
    /// records what it was asked.
    #[derive(Default)]
    struct ScriptedSandbox {
        outputs: Mutex<Vec<ExecOutput>>,
        commands: Mutex<Vec<String>>,
        uploads: Mutex<Vec<(String, Vec<u8>)>>,
    }

    impl ScriptedSandbox {
        fn with(outputs: Vec<ExecOutput>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into_iter().rev().collect()),
                ..Self::default()
            }
        }

        fn commands(&self) -> Vec<String> {
            self.commands
                .lock()
                .map(|commands| commands.clone())
                .unwrap_or_default()
        }
    }

    #[async_trait]
    impl Sandbox for ScriptedSandbox {
        async fn exec(&self, request: ExecRequest) -> Result<ExecOutput> {
            if let Ok(mut commands) = self.commands.lock() {
                commands.push(request.command.last().cloned().unwrap_or_default());
            }
            self.outputs
                .lock()
                .ok()
                .and_then(|mut outputs| outputs.pop())
                .context("no scripted output left")
        }

        async fn upload_tar(&self, destination: &str, archive: Vec<u8>) -> Result<()> {
            if let Ok(mut uploads) = self.uploads.lock() {
                uploads.push((destination.to_owned(), archive));
            }
            Ok(())
        }
    }

    fn stdout(text: &str) -> ExecOutput {
        ExecOutput {
            stdout: text.into(),
            ..ExecOutput::default()
        }
    }

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "call".into(),
            name: name.into(),
            arguments,
        }
    }

    #[tokio::test]
    async fn read_file_numbers_lines_and_points_at_the_rest() {
        let sandbox = ScriptedSandbox::with(vec![stdout("__GRIDOPS_TOTAL__ 3\nfirst\nsecond\n")]);
        let result = execute(
            &sandbox,
            &call("read_file", json!({ "path": "src/a.rs", "end_line": 2 })),
        )
        .await;
        assert!(!result.is_error);
        assert!(result.text.starts_with("src/a.rs (lines 1-2 of 3)"));
        assert!(result.text.contains("     1\tfirst"));
        assert!(result.text.contains("Read from line 3"));
    }

    #[tokio::test]
    async fn bad_paths_and_unknown_tools_become_errors_for_the_model() {
        let sandbox = ScriptedSandbox::default();
        let escaped = execute(&sandbox, &call("read_file", json!({ "path": "../x" }))).await;
        assert!(escaped.is_error);
        assert!(escaped.text.starts_with("Error: Paths may not leave"));
        let unknown = execute(&sandbox, &call("delete_repo", json!({}))).await;
        assert!(unknown.is_error);
        assert!(sandbox.commands().is_empty());
    }

    #[tokio::test]
    async fn edit_file_requires_a_unique_match() {
        let sandbox = ScriptedSandbox::with(vec![stdout("let a = 1;\nlet a = 1;\n")]);
        let result = execute(
            &sandbox,
            &call(
                "edit_file",
                json!({ "path": "a.rs", "old_text": "let a = 1;", "new_text": "let a = 2;" }),
            ),
        )
        .await;
        assert!(result.is_error);
        assert!(result.text.contains("appears 2 times"));
    }

    #[tokio::test]
    async fn edit_file_uploads_and_writes_through_the_scratch_file() -> Result<()> {
        let sandbox = ScriptedSandbox::with(vec![stdout("fn main() {}\n"), stdout("")]);
        let result = execute(
            &sandbox,
            &call(
                "edit_file",
                json!({ "path": "src/main.rs", "old_text": "fn main() {}", "new_text": "fn main() { run(); }" }),
            ),
        )
        .await;
        assert!(!result.is_error, "{}", result.text);
        let uploads = sandbox
            .uploads
            .lock()
            .map(|uploads| uploads.clone())
            .unwrap_or_default();
        assert_eq!(uploads.len(), 1);
        assert_eq!(uploads[0].0, SCRATCH_ROOT);
        let mut archive = tar::Archive::new(uploads[0].1.as_slice());
        let mut entry = archive.entries()?.next().context("entry")??;
        let mut written = String::new();
        std::io::Read::read_to_string(&mut entry, &mut written)?;
        assert_eq!(written, "fn main() { run(); }\n");
        assert!(sandbox.commands()[1].contains("> 'src/main.rs'"));
        Ok(())
    }

    #[tokio::test]
    async fn search_reports_no_matches_without_error() {
        let sandbox = ScriptedSandbox::with(vec![ExecOutput {
            exit_code: 1,
            ..ExecOutput::default()
        }]);
        let result = execute(&sandbox, &call("search", json!({ "pattern": "nothing" }))).await;
        assert!(!result.is_error);
        assert_eq!(result.text, "No matches.");
    }

    #[tokio::test]
    async fn failing_commands_are_results_not_errors() {
        let sandbox = ScriptedSandbox::with(vec![ExecOutput {
            exit_code: 1,
            stderr: "error[E0425]: cannot find value".into(),
            duration_ms: 1_500,
            ..ExecOutput::default()
        }]);
        let result = execute(
            &sandbox,
            &call("run_command", json!({ "command": "cargo build" })),
        )
        .await;
        assert!(!result.is_error);
        assert_eq!(result.title, "Run cargo build → exit 1");
        assert!(result.text.contains("exit code: 1 (1.5s)"));
        assert!(result.text.contains("E0425"));
    }

    #[test]
    fn listings_fold_deep_directories() {
        let files = ["README.md", "src/lib.rs", "src/a/b/c.rs", "src/a/b/d.rs"];
        let text = summarize_listing(".", files.iter().copied(), 2);
        assert_eq!(text, "README.md\nsrc/a/ (2 more files)\nsrc/lib.rs");
        let scoped = summarize_listing("src", files.iter().copied(), 1);
        assert_eq!(scoped, "src/a/ (2 more files)\nsrc/lib.rs");
    }

    #[test]
    fn finish_requests_are_validated() {
        let ok = FinishRequest::parse(&json!({
            "outcome": "diagnosis", "title": "Runner ran out of memory",
            "summary": "Node aborted.", "body": "Raise the pool memory."
        }));
        assert!(ok.is_ok_and(|request| request.outcome == FinishOutcome::Diagnosis));
        assert!(FinishRequest::parse(&json!({ "outcome": "merge" })).is_err());
        assert!(
            FinishRequest::parse(&json!({
                "outcome": "pull_request", "title": " ", "summary": "x", "body": "y"
            }))
            .is_err()
        );
    }

    #[test]
    fn middle_clipping_keeps_the_end() {
        let value = format!("{}END", "x".repeat(100));
        let clipped = clip_middle(&value, 20);
        assert!(clipped.ends_with("END"));
        assert!(clipped.contains("characters omitted"));
    }
}
