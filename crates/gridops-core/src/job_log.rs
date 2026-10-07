//! Turns a raw GitHub Actions job log into steps, clean console lines, and
//! annotations. Shared by the API's log reader and the fix agent's failure
//! context, so both see the same failed step and the same error lines.

use std::collections::HashSet;

use chrono::SecondsFormat;

use crate::GitHubWorkflowStep;

fn iso_optional(value: Option<i64>) -> Option<String> {
    value
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Millis, true))
}

#[derive(Debug)]
pub struct ParsedJobLog {
    pub steps: Vec<StructuredLogStep>,
    pub annotations: Vec<LogAnnotation>,
    pub line_count: usize,
    pub hidden_diagnostic_lines: usize,
}

#[derive(Debug)]
pub struct StructuredLogStep {
    pub number: i64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub lines: Vec<CleanLogLine>,
}

#[derive(Clone, Debug)]
pub struct CleanLogLine {
    pub timestamp: Option<String>,
    pub timestamp_millis: Option<i64>,
    pub text: String,
    pub level: &'static str,
}

#[derive(Debug)]
pub struct LogAnnotation {
    pub level: &'static str,
    pub message: String,
    pub step_number: i64,
    pub step_name: String,
}

pub fn structure_job_log(raw: &str, metadata: &[GitHubWorkflowStep], local: bool) -> ParsedJobLog {
    let (lines, hidden_diagnostic_lines) = clean_job_lines(raw, local);
    let mut steps = metadata
        .iter()
        .map(|step| StructuredLogStep {
            number: step.number,
            name: step.name.clone(),
            status: step.status.clone(),
            conclusion: step.conclusion.clone(),
            started_at: step.started_at.clone(),
            completed_at: step.completed_at.clone(),
            lines: Vec::new(),
        })
        .collect::<Vec<_>>();
    if steps.is_empty() {
        steps.push(StructuredLogStep {
            number: 1,
            name: "Job output".into(),
            status: "completed".into(),
            conclusion: None,
            started_at: None,
            completed_at: None,
            lines: Vec::new(),
        });
    }
    let starts = steps
        .iter()
        .map(|step| github_date_millis(step.started_at.as_deref()))
        .collect::<Vec<_>>();
    for line in lines {
        let index = line.timestamp_millis.map_or(0, |timestamp| {
            starts
                .iter()
                .enumerate()
                .filter_map(|(index, start)| start.map(|start| (index, start)))
                .filter(|(_, start)| *start <= timestamp)
                .map(|(index, _)| index)
                .next_back()
                .unwrap_or(0)
        });
        steps[index].lines.push(line);
    }
    // GitHub exposes step timestamps with second precision. The final error for
    // a failed step can therefore appear a few milliseconds after cleanup steps
    // report the same start second. Keep that error with the step GitHub marked
    // as failed instead of presenting it under "Complete job".
    let failed_ranges = steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.conclusion.as_deref() == Some("failure"))
        .map(|(index, step)| {
            (
                index,
                github_date_millis(step.started_at.as_deref()),
                github_date_millis(step.completed_at.as_deref()),
            )
        })
        .collect::<Vec<_>>();
    let mut reassigned_errors = Vec::new();
    for step in &mut steps {
        if step.conclusion.as_deref() == Some("failure") {
            continue;
        }
        let mut retained = Vec::new();
        for line in std::mem::take(&mut step.lines) {
            let target = if line.level == "error" {
                line.timestamp_millis.and_then(|timestamp| {
                    failed_ranges
                        .iter()
                        .filter(|(_, started_at, completed_at)| {
                            started_at.is_none_or(|started_at| started_at <= timestamp)
                                && completed_at.is_none_or(|completed_at| {
                                    timestamp <= completed_at.saturating_add(1_000)
                                })
                        })
                        .map(|(index, _, _)| *index)
                        .next_back()
                })
            } else {
                None
            };
            if let Some(target) = target {
                reassigned_errors.push((target, line));
            } else {
                retained.push(line);
            }
        }
        step.lines = retained;
    }
    for (target, line) in reassigned_errors {
        steps[target].lines.push(line);
    }
    for step in &mut steps {
        step.lines
            .sort_by_key(|line| line.timestamp_millis.unwrap_or(i64::MIN));
    }
    let mut annotations = Vec::new();
    let mut annotation_keys = HashSet::new();
    for step in &steps {
        for line in &step.lines {
            if !matches!(line.level, "error" | "warning") {
                continue;
            }
            let key = format!("{}:{}:{}", line.level, step.number, line.text);
            if annotation_keys.insert(key) {
                annotations.push(LogAnnotation {
                    level: line.level,
                    message: line.text.clone(),
                    step_number: step.number,
                    step_name: step.name.clone(),
                });
            }
        }
        if step.conclusion.as_deref() == Some("failure")
            && !annotations.iter().any(|annotation| {
                annotation.step_number == step.number && annotation.level == "error"
            })
        {
            let message = step
                .lines
                .iter()
                .rev()
                .find(|line| line.text.to_ascii_lowercase().contains("error"))
                .map_or_else(
                    || "Step concluded with failure.".into(),
                    |line| line.text.clone(),
                );
            annotations.push(LogAnnotation {
                level: "error",
                message,
                step_number: step.number,
                step_name: step.name.clone(),
            });
        }
    }
    let line_count = steps.iter().map(|step| step.lines.len()).sum();
    ParsedJobLog {
        steps,
        annotations,
        line_count,
        hidden_diagnostic_lines,
    }
}

pub fn clean_job_lines(raw: &str, local: bool) -> (Vec<CleanLogLine>, usize) {
    let mut lines = Vec::new();
    let mut seen = HashSet::new();
    let mut hidden = 0;
    for physical_line in raw.lines() {
        let without_ansi = strip_ansi_sequences(physical_line.trim_end_matches('\r'));
        let (outer_timestamp, outer_body) = split_log_timestamp(&without_ansi);
        let mut body = outer_body.trim_start_matches('\u{feff}');
        let mut timestamp = outer_timestamp;
        if local {
            let (inner_timestamp, inner_body) = split_log_timestamp(body);
            let Some(inner_timestamp) = inner_timestamp else {
                hidden += usize::from(!body.trim().is_empty());
                continue;
            };
            timestamp = Some(inner_timestamp);
            body = inner_body;
        }
        if body.starts_with("[RUNNER ") || body.starts_with("[WORKER ") {
            hidden += 1;
            continue;
        }
        if let Some(index) = body
            .find("[WORKER ")
            .into_iter()
            .chain(body.find("[RUNNER "))
            .min()
        {
            body = body[..index].trim_end();
            hidden += 1;
        }
        let Some((level, text)) = classify_console_line(body) else {
            hidden += 1;
            continue;
        };
        if text.is_empty() && level != "output" {
            continue;
        }
        let timestamp_string = iso_optional(timestamp);
        let dedupe_timestamp = timestamp;
        let key = format!("{dedupe_timestamp:?}:{level}:{text}");
        if !seen.insert(key) {
            continue;
        }
        lines.push(CleanLogLine {
            timestamp: timestamp_string,
            timestamp_millis: timestamp,
            text,
            level,
        });
    }
    lines.sort_by_key(|line| line.timestamp_millis.unwrap_or(i64::MIN));
    (lines, hidden)
}

fn classify_console_line(value: &str) -> Option<(&'static str, String)> {
    let value = value.trim_end();
    if value == "##[endgroup]" || value.starts_with("##[debug]") {
        return None;
    }
    for (command, level) in [
        ("group", "group"),
        ("error", "error"),
        ("warning", "warning"),
        ("notice", "notice"),
        ("command", "command"),
        ("section", "group"),
    ] {
        if let Some(text) = github_command_message(value, command) {
            return Some((level, text.to_owned()));
        }
    }
    Some(("output", value.to_owned()))
}

fn github_command_message<'a>(value: &'a str, command: &str) -> Option<&'a str> {
    let rest = value.strip_prefix(&format!("##[{command}"))?;
    let (_, message) = rest.split_once(']')?;
    Some(message)
}

fn split_log_timestamp(value: &str) -> (Option<i64>, &str) {
    let value = value.trim_start_matches('\u{feff}');
    let Some((candidate, rest)) = value.split_once(' ') else {
        return (None, value);
    };
    let timestamp = chrono::DateTime::parse_from_rfc3339(candidate)
        .ok()
        .map(|date| date.timestamp_millis());
    match timestamp {
        Some(timestamp) => (Some(timestamp), rest),
        None => (None, value),
    }
}

pub fn github_date_millis(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|date| date.timestamp_millis())
}

pub fn strip_ansi_sequences(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            output.push(character);
            continue;
        }
        if characters.next_if_eq(&'[').is_none() {
            continue;
        }
        for next in characters.by_ref() {
            if next.is_ascii_alphabetic() {
                break;
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_job_logs_remove_runner_noise_and_surface_the_failed_step() {
        let steps = vec![
            GitHubWorkflowStep {
                name: "Run checkout".into(),
                status: "completed".into(),
                conclusion: Some("success".into()),
                number: 1,
                started_at: Some("2026-07-21T19:38:58.000Z".into()),
                completed_at: Some("2026-07-21T19:39:00.000Z".into()),
            },
            GitHubWorkflowStep {
                name: "Install pnpm".into(),
                status: "completed".into(),
                conclusion: Some("failure".into()),
                number: 2,
                started_at: Some("2026-07-21T19:39:00.000Z".into()),
                completed_at: Some("2026-07-21T19:39:02.000Z".into()),
            },
        ];
        let raw = concat!(
            "2026-07-21T19:38:57.000000000Z [WORKER 2026-07-21 19:38:57Z INFO JobServerQueue] Uploading logs\n",
            "2026-07-21T19:38:58.000000000Z [GRIDOPS JOB LOG page.log]\n",
            "2026-07-21T19:38:58.100000000Z 2026-07-21T19:38:58.0104000Z ##[group]Run actions/checkout@v5\n",
            "2026-07-21T19:38:58.200000000Z 2026-07-21T19:38:58.2000000Z Checked out repository\n",
            "2026-07-21T19:39:00.100000000Z 2026-07-21T19:39:00.0104000Z ##[group]Run pnpm/action-setup@v4\n",
            "2026-07-21T19:39:01.100000000Z 2026-07-21T19:39:01.0104000Z Error: No pnpm version is specified.\n",
            "2026-07-21T19:39:01.200000000Z 2026-07-21T19:39:01.0204000Z ##[error]Process completed with exit code 1.\n",
            "2026-07-21T19:39:01.300000000Z 2026-07-21T19:39:01.0204999Z ##[error]Process completed with exit code 1.\n",
        );

        let parsed = structure_job_log(raw, &steps, true);

        assert_eq!(parsed.steps.len(), 2);
        assert_eq!(parsed.steps[0].lines.len(), 2);
        assert_eq!(parsed.steps[1].lines.len(), 3);
        assert_eq!(parsed.annotations.len(), 1);
        assert_eq!(parsed.annotations[0].step_name, "Install pnpm");
        assert_eq!(
            parsed.annotations[0].message,
            "Process completed with exit code 1."
        );
        assert!(parsed.hidden_diagnostic_lines >= 2);
        assert!(
            parsed
                .steps
                .iter()
                .flat_map(|step| &step.lines)
                .all(|line| {
                    !line.text.contains("JobServerQueue") && !line.text.contains("GRIDOPS JOB LOG")
                })
        );
    }

    #[test]
    fn github_job_logs_keep_clean_console_lines_and_remove_ansi_sequences() {
        let steps = vec![GitHubWorkflowStep {
            name: "Tests".into(),
            status: "completed".into(),
            conclusion: Some("success".into()),
            number: 1,
            started_at: Some("2026-07-21T19:40:00.000Z".into()),
            completed_at: Some("2026-07-21T19:40:01.000Z".into()),
        }];
        let raw = "2026-07-21T19:40:00.100Z \u{1b}[32m22 tests passed\u{1b}[0m\n";

        let parsed = structure_job_log(raw, &steps, false);

        assert_eq!(parsed.line_count, 1);
        assert_eq!(parsed.steps[0].lines[0].text, "22 tests passed");
    }

    #[test]
    fn final_error_stays_with_the_failed_step_when_cleanup_shares_its_second() {
        let steps = vec![
            GitHubWorkflowStep {
                name: "Verify runner toolchain".into(),
                status: "completed".into(),
                conclusion: Some("failure".into()),
                number: 4,
                started_at: Some("2026-07-21T19:39:01Z".into()),
                completed_at: Some("2026-07-21T19:39:48Z".into()),
            },
            GitHubWorkflowStep {
                name: "Complete job".into(),
                status: "completed".into(),
                conclusion: Some("success".into()),
                number: 17,
                started_at: Some("2026-07-21T19:39:48Z".into()),
                completed_at: Some("2026-07-21T19:39:48Z".into()),
            },
        ];
        let raw = concat!(
            "2026-07-21T19:39:47.900Z rustup could not install the toolchain\n",
            "2026-07-21T19:39:48.385Z ##[error]Process completed with exit code 1.\n",
            "2026-07-21T19:39:48.667Z Cleaning up orphan processes\n",
        );

        let parsed = structure_job_log(raw, &steps, false);

        assert_eq!(parsed.annotations.len(), 1);
        assert_eq!(parsed.annotations[0].step_name, "Verify runner toolchain");
        assert_eq!(parsed.steps[0].lines.len(), 2);
        assert_eq!(parsed.steps[1].lines.len(), 1);
        assert_eq!(parsed.steps[0].lines[1].level, "error");
    }
}
