//! The fix agent's loop: ask the model, run its tools in the sandbox, repeat
//! until it calls `finish` or runs out of turns or time.
//!
//! agent-runtime runs a single round of tool calls, so the loop lives here.
//! It guarantees every tool call gets a result, since providers reject a
//! history with an unanswered call, and it never lets a tool error end the run.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use agent_runtime::{AssistantTurn, ChatMessage, MessageRole, ToolDefinition};
use anyhow::Result;
use async_trait::async_trait;
use secrecy::SecretString;

use crate::{
    PROVIDER_HTTP_TIMEOUT_SECONDS,
    changes::{ChangeKind, changed_paths},
    connection::AgentModel,
    prompt::{FailureContext, system_prompt, task_message},
    sandbox::{Sandbox, repo_relative_path},
    tools::{FINISH_TOOL, FinishOutcome, FinishRequest, ToolResult, clip, definitions, execute},
};

/// Characters of history kept before older tool output is dropped. Roughly
/// 100k tokens, well inside every supported model's window.
const CONTEXT_BUDGET_CHARS: usize = 400_000;
/// Recent messages that keep their tool output when compacting.
const KEEP_RECENT_MESSAGES: usize = 12;
const ELIDED: &str = "[Earlier output removed to save space. Run the tool again if you need it.]";
const MAX_TURN_ATTEMPTS: u32 = 3;
const MAX_IDLE_TURNS: usize = 2;

/// The model, as the loop needs it. Implemented by [`AgentModel`]; tests
/// script it.
#[async_trait]
pub trait TurnModel: Send + Sync {
    async fn turn(
        &self,
        system_prompt: &str,
        history: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<AssistantTurn>;

    async fn refreshed_credential(&self) -> Result<Option<SecretString>> {
        Ok(None)
    }
}

#[async_trait]
impl TurnModel for AgentModel {
    async fn turn(
        &self,
        system_prompt: &str,
        history: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<AssistantTurn> {
        AgentModel::turn(self, system_prompt, history, tools).await
    }

    async fn refreshed_credential(&self) -> Result<Option<SecretString>> {
        AgentModel::refreshed_credential(self).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Status,
    Message,
    Tool,
    Result,
    Error,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Message => "message",
            Self::Tool => "tool",
            Self::Result => "result",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEvent {
    pub kind: EventKind,
    pub title: String,
    pub detail: Option<String>,
}

impl AgentEvent {
    pub fn new(kind: EventKind, title: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            detail,
        }
    }
}

/// Where the loop reports progress, and how it learns it should stop.
#[async_trait]
pub trait RunObserver: Send + Sync {
    async fn record(&self, event: AgentEvent);

    async fn cancelled(&self) -> bool;

    /// A subscription refreshed its tokens; persist them before the old
    /// refresh token is needed again.
    async fn credential_refreshed(&self, secret: SecretString);
}

#[derive(Debug, Clone, Copy)]
pub struct SessionLimits {
    pub max_turns: usize,
    pub max_duration: Duration,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            max_turns: 40,
            max_duration: Duration::from_mins(25),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentOutcome {
    pub finish: FinishRequest,
    pub turns: usize,
    pub tool_calls: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("The run was cancelled.")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

/// Runs the agent against one failure.
pub async fn run_session(
    model: &dyn TurnModel,
    sandbox: &dyn Sandbox,
    context: &FailureContext,
    limits: SessionLimits,
    observer: &dyn RunObserver,
) -> Result<AgentOutcome, SessionError> {
    let started = Instant::now();
    let system = system_prompt(limits.max_turns);
    let tools = definitions();
    let mut history = vec![ChatMessage::user(task_message(context))];
    let mut tool_calls = 0;
    let mut idle_turns = 0;
    let mut edited = BTreeSet::new();
    let mut side_effects_confirmed = false;
    let mut warned_about_budget = false;

    for turn_index in 0..limits.max_turns {
        if observer.cancelled().await {
            return Err(SessionError::Cancelled);
        }
        if started.elapsed() >= limits.max_duration {
            return Err(SessionError::Failed(format!(
                "The agent did not finish within {} minutes.",
                limits.max_duration.as_secs() / 60
            )));
        }
        let remaining = limits.max_turns - turn_index;
        if remaining <= 4 && !warned_about_budget {
            warned_about_budget = true;
            history.push(ChatMessage::user(format!(
                "You have {remaining} turns left. Wrap up now: finish with your fix, or with a diagnosis of what you found."
            )));
        }
        compact(&mut history, CONTEXT_BUDGET_CHARS);

        let turn = request_turn(model, &system, &history, &tools, observer).await?;
        if let Ok(Some(secret)) = model.refreshed_credential().await {
            observer.credential_refreshed(secret).await;
        }
        if let Some(text) = turn
            .content
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            observer
                .record(AgentEvent::new(
                    EventKind::Message,
                    clip(text.lines().next().unwrap_or(text), 160),
                    Some(clip(text, 8_000)),
                ))
                .await;
        }
        if turn.tool_calls.is_empty() {
            idle_turns += 1;
            if idle_turns > MAX_IDLE_TURNS {
                return Err(SessionError::Failed(
                    "The model stopped without calling finish.".into(),
                ));
            }
            history.push(ChatMessage::assistant(turn.content.unwrap_or_default()));
            history.push(ChatMessage::user(
                "Keep going with the tools. When you are done, call finish.",
            ));
            continue;
        }
        idle_turns = 0;
        history.push(ChatMessage::assistant_with_tools(
            turn.content.clone(),
            turn.tool_calls.clone(),
        ));

        let mut finished = None;
        for call in &turn.tool_calls {
            tool_calls += 1;
            if call.name == FINISH_TOOL {
                let reply = match FinishRequest::parse(&call.arguments) {
                    Err(error) => format!("Error: {error}"),
                    Ok(_) if finished.is_some() => {
                        "Error: finish was already called this turn.".to_owned()
                    }
                    Ok(request) => {
                        match check_finish(sandbox, &request, &edited, &mut side_effects_confirmed)
                            .await
                        {
                            Ok(()) => {
                                finished = Some(request);
                                "Recorded. GridOps will take it from here.".to_owned()
                            }
                            Err(message) => message,
                        }
                    }
                };
                history.push(ChatMessage::tool(call.id.clone(), reply));
                continue;
            }
            if observer.cancelled().await {
                return Err(SessionError::Cancelled);
            }
            let result = execute(sandbox, call).await;
            if !result.is_error
                && matches!(call.name.as_str(), "edit_file" | "write_file")
                && let Some(path) = call
                    .arguments
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|path| repo_relative_path(path).ok())
            {
                edited.insert(path);
            }
            record_tool(observer, &result).await;
            history.push(ChatMessage::tool(call.id.clone(), result.text));
        }
        if let Some(finish) = finished {
            return Ok(AgentOutcome {
                finish,
                turns: turn_index + 1,
                tool_calls,
            });
        }
    }
    Err(SessionError::Failed(format!(
        "The agent used all {} turns without finishing.",
        limits.max_turns
    )))
}

async fn request_turn(
    model: &dyn TurnModel,
    system: &str,
    history: &[ChatMessage],
    tools: &[ToolDefinition],
    observer: &dyn RunObserver,
) -> Result<AssistantTurn, SessionError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let result = tokio::time::timeout(
            Duration::from_secs(PROVIDER_HTTP_TIMEOUT_SECONDS + 30),
            model.turn(system, history, tools),
        )
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("The model request timed out.")));
        match result {
            Ok(turn) => return Ok(turn),
            Err(error) if attempt < MAX_TURN_ATTEMPTS && transient(&error) => {
                let wait = Duration::from_secs(10 * u64::from(attempt));
                observer
                    .record(AgentEvent::new(
                        EventKind::Status,
                        format!("Model busy; retrying in {}s", wait.as_secs()),
                        Some(clip(&error.to_string(), 500)),
                    ))
                    .await;
                tokio::time::sleep(wait).await;
                if observer.cancelled().await {
                    return Err(SessionError::Cancelled);
                }
            }
            Err(error) => {
                return Err(SessionError::Failed(format!(
                    "The model request failed: {}",
                    clip(&format!("{error:#}"), 600)
                )));
            }
        }
    }
}

fn transient(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "429",
        "500",
        "502",
        "503",
        "504",
        "529",
        "overloaded",
        "temporarily",
        "timed out",
        "timeout",
        "connection reset",
        "rate limit",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

async fn record_tool(observer: &dyn RunObserver, result: &ToolResult) {
    observer
        .record(AgentEvent::new(
            if result.is_error {
                EventKind::Error
            } else {
                EventKind::Tool
            },
            result.title.clone(),
            result.detail.clone(),
        ))
        .await;
}

/// Accepts a finish, or explains to the model why not yet. A pull request
/// needs changes, and files changed only as a side effect of commands (lock
/// files, build output) are shown to the model once so it can keep or revert
/// them deliberately.
async fn check_finish(
    sandbox: &dyn Sandbox,
    request: &FinishRequest,
    edited: &BTreeSet<String>,
    side_effects_confirmed: &mut bool,
) -> Result<(), String> {
    if request.outcome == FinishOutcome::Diagnosis {
        return Ok(());
    }
    let changed = changed_paths(sandbox)
        .await
        .map_err(|error| format!("Error: could not read your changes: {error}"))?;
    if changed.is_empty() {
        return Err(
            "Error: no files have changed. Make the fix first, or finish with outcome diagnosis."
                .into(),
        );
    }
    let unexpected = changed
        .iter()
        .filter(|change| !edited.contains(&change.path))
        .map(|change| {
            format!(
                "{} ({})",
                change.path,
                match change.kind {
                    ChangeKind::Added => "added",
                    ChangeKind::Modified => "modified",
                    ChangeKind::Deleted => "deleted",
                }
            )
        })
        .collect::<Vec<_>>();
    if unexpected.is_empty() || *side_effects_confirmed {
        return Ok(());
    }
    *side_effects_confirmed = true;
    Err(format!(
        "Not finished yet. These files changed without edit_file or write_file, probably as a side effect of commands you ran:\n{}\n\nIf they belong in the fix, call finish again. Otherwise restore them (git checkout -- <path>, or delete new files) and then call finish.",
        unexpected
            .iter()
            .take(40)
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

/// Drops the output of older tool calls once the history grows past the
/// budget. The opening brief and the most recent exchanges are kept whole.
fn compact(history: &mut [ChatMessage], budget: usize) {
    let size = |history: &[ChatMessage]| -> usize {
        history
            .iter()
            .map(|message| {
                message.content.as_deref().map_or(0, str::len)
                    + message
                        .tool_calls
                        .iter()
                        .map(|call| call.arguments.to_string().len())
                        .sum::<usize>()
            })
            .sum()
    };
    let mut total = size(history);
    if total <= budget {
        return;
    }
    let protected_from = history.len().saturating_sub(KEEP_RECENT_MESSAGES);
    for message in history.iter_mut().take(protected_from).skip(1) {
        if total <= budget {
            break;
        }
        if message.role != MessageRole::Tool {
            continue;
        }
        let Some(content) = message.content.as_deref() else {
            continue;
        };
        if content == ELIDED || content.len() < 500 {
            continue;
        }
        total -= content.len() - ELIDED.len();
        message.content = Some(ELIDED.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use agent_runtime::ToolCall;
    use anyhow::Context as _;
    use serde_json::{Value, json};

    use super::*;
    use crate::sandbox::{ExecOutput, ExecRequest};

    struct ScriptedModel {
        turns: Mutex<Vec<Result<AssistantTurn>>>,
        seen: Mutex<Vec<Vec<ChatMessage>>>,
    }

    impl ScriptedModel {
        fn new(turns: Vec<Result<AssistantTurn>>) -> Self {
            Self {
                turns: Mutex::new(turns.into_iter().rev().collect()),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl TurnModel for ScriptedModel {
        async fn turn(
            &self,
            _system: &str,
            history: &[ChatMessage],
            _tools: &[ToolDefinition],
        ) -> Result<AssistantTurn> {
            if let Ok(mut seen) = self.seen.lock() {
                seen.push(history.to_vec());
            }
            self.turns
                .lock()
                .ok()
                .and_then(|mut turns| turns.pop())
                .context("script exhausted")?
        }
    }

    /// Reports no changes until a file has been written, then `diff_after_write`.
    struct FakeSandbox {
        diff_after_write: String,
        commands: Mutex<Vec<String>>,
    }

    impl FakeSandbox {
        fn new(diff_after_write: &str) -> Self {
            Self {
                diff_after_write: diff_after_write.to_owned(),
                commands: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Sandbox for FakeSandbox {
        async fn exec(&self, request: ExecRequest) -> Result<ExecOutput> {
            let script = request.command.last().cloned().unwrap_or_default();
            let wrote = {
                let mut commands = self
                    .commands
                    .lock()
                    .map_err(|_| anyhow::anyhow!("poisoned"))?;
                commands.push(script.clone());
                commands.iter().any(|command| command.contains(" > '"))
            };
            if script.contains("git diff --cached --raw") {
                return Ok(ExecOutput {
                    stdout: if wrote {
                        self.diff_after_write.clone()
                    } else {
                        String::new()
                    },
                    ..ExecOutput::default()
                });
            }
            if script.contains("cat --") {
                return Ok(ExecOutput {
                    stdout: "old\n".into(),
                    ..ExecOutput::default()
                });
            }
            Ok(ExecOutput::default())
        }

        async fn upload_tar(&self, _destination: &str, _archive: Vec<u8>) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<AgentEvent>>,
        cancel: Mutex<bool>,
    }

    #[async_trait]
    impl RunObserver for Recorder {
        async fn record(&self, event: AgentEvent) {
            if let Ok(mut events) = self.events.lock() {
                events.push(event);
            }
        }

        async fn cancelled(&self) -> bool {
            self.cancel.lock().map(|cancel| *cancel).unwrap_or(false)
        }

        async fn credential_refreshed(&self, _secret: SecretString) {}
    }

    fn calls(calls: &[(&str, &str, Value)]) -> Result<AssistantTurn> {
        Ok(AssistantTurn {
            content: None,
            tool_calls: calls
                .iter()
                .map(|(id, name, arguments)| ToolCall {
                    id: (*id).into(),
                    name: (*name).into(),
                    arguments: arguments.clone(),
                })
                .collect(),
        })
    }

    fn finish(outcome: &str) -> Value {
        json!({ "outcome": outcome, "title": "Fix the build", "summary": "s", "body": "b" })
    }

    fn limits() -> SessionLimits {
        SessionLimits {
            max_turns: 6,
            max_duration: Duration::from_mins(1),
        }
    }

    #[tokio::test]
    async fn a_diagnosis_finishes_and_every_call_gets_a_result() -> Result<()> {
        let model = ScriptedModel::new(vec![calls(&[
            ("1", "run_command", json!({ "command": "free -m" })),
            ("2", FINISH_TOOL, finish("diagnosis")),
        ])]);
        let sandbox = FakeSandbox::new("");
        let observer = Recorder::default();
        let outcome = run_session(
            &model,
            &sandbox,
            &FailureContext::default(),
            limits(),
            &observer,
        )
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
        assert_eq!(outcome.finish.outcome, FinishOutcome::Diagnosis);
        assert_eq!(outcome.turns, 1);
        assert_eq!(outcome.tool_calls, 2);
        let events = observer
            .events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default();
        assert!(
            events
                .iter()
                .any(|event| event.title.starts_with("Run free -m"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_pull_request_needs_changes_and_side_effects_are_confirmed_once() -> Result<()> {
        let model = ScriptedModel::new(vec![
            calls(&[("1", FINISH_TOOL, finish("pull_request"))]),
            calls(&[(
                "2",
                "edit_file",
                json!({ "path": "src/a.rs", "old_text": "old", "new_text": "new" }),
            )]),
            calls(&[("3", FINISH_TOOL, finish("pull_request"))]),
            calls(&[("4", FINISH_TOOL, finish("pull_request"))]),
        ]);
        // No changes yet, so the first finish is refused. After the edit the
        // diff shows the edited file plus a lock file the model never touched.
        let sandbox = FakeSandbox::new(concat!(
            ":100644 100644 a b M\0src/a.rs\0",
            ":100644 100644 c d M\0Cargo.lock\0",
        ));
        let observer = Recorder::default();
        let outcome = run_session(
            &model,
            &sandbox,
            &FailureContext::default(),
            limits(),
            &observer,
        )
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
        assert_eq!(outcome.finish.outcome, FinishOutcome::PullRequest);
        assert_eq!(outcome.turns, 4);
        let seen = model
            .seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default();
        let replies = seen
            .last()
            .context("history")?
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .filter_map(|message| message.content.clone())
            .collect::<Vec<_>>();
        assert!(replies[0].contains("no files have changed"));
        assert!(replies[2].contains("Cargo.lock (modified)"));
        assert!(!replies[2].contains("src/a.rs"));
        Ok(())
    }

    #[tokio::test]
    async fn the_loop_stops_when_the_model_goes_quiet() {
        let quiet = || {
            Ok(AssistantTurn {
                content: Some("Thinking about it.".into()),
                tool_calls: Vec::new(),
            })
        };
        let model = ScriptedModel::new(vec![quiet(), quiet(), quiet()]);
        let result = run_session(
            &model,
            &FakeSandbox::new(""),
            &FailureContext::default(),
            limits(),
            &Recorder::default(),
        )
        .await;
        assert!(
            matches!(result, Err(SessionError::Failed(message)) if message.contains("without calling finish"))
        );
    }

    #[tokio::test]
    async fn cancellation_is_honoured_between_turns() {
        let model = ScriptedModel::new(vec![]);
        let observer = Recorder::default();
        if let Ok(mut cancel) = observer.cancel.lock() {
            *cancel = true;
        }
        let result = run_session(
            &model,
            &FakeSandbox::new(""),
            &FailureContext::default(),
            limits(),
            &observer,
        )
        .await;
        assert!(matches!(result, Err(SessionError::Cancelled)));
    }

    #[tokio::test]
    async fn turn_limits_end_the_run() {
        let model = ScriptedModel::new(
            (0..6)
                .map(|index| {
                    calls(&[(
                        &*format!("{index}"),
                        "run_command",
                        json!({ "command": "true" }),
                    )])
                })
                .collect(),
        );
        let result = run_session(
            &model,
            &FakeSandbox::new(""),
            &FailureContext::default(),
            limits(),
            &Recorder::default(),
        )
        .await;
        assert!(
            matches!(result, Err(SessionError::Failed(message)) if message.contains("all 6 turns"))
        );
        let seen = model
            .seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default();
        let warned = seen.iter().flatten().any(|message| {
            message
                .content
                .as_deref()
                .is_some_and(|text| text.contains("turns left"))
        });
        assert!(warned);
    }

    #[test]
    fn compaction_drops_old_tool_output_first() {
        let big = "x".repeat(2_000);
        let mut history = vec![ChatMessage::user(big.clone())];
        for index in 0..20 {
            history.push(ChatMessage::tool(format!("{index}"), big.clone()));
        }
        compact(&mut history, 20_000);
        assert_eq!(history[0].content.as_deref(), Some(big.as_str()));
        assert_eq!(history[1].content.as_deref(), Some(ELIDED));
        assert_eq!(history[20].content.as_deref(), Some(big.as_str()));
    }

    #[test]
    fn rate_limits_and_outages_are_retried() {
        assert!(transient(&anyhow::anyhow!(
            "Claude Code is temporarily unavailable (529)"
        )));
        assert!(transient(&anyhow::anyhow!(
            "request failed (429 Too Many Requests)"
        )));
        assert!(!transient(&anyhow::anyhow!("invalid x-api-key (401)")));
    }
}
