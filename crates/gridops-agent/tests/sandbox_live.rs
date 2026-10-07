//! Drives a real runner-manager sandbox with the agent's own tools.
//!
//! Ignored by default. Start a manager against a Docker engine, then run:
//!
//! ```text
//! GRIDOPS_SMOKE_MANAGER_URL=http://127.0.0.1:8788/ \
//! GRIDOPS_SMOKE_MANAGER_TOKEN=... GRIDOPS_SMOKE_NETWORK=gridops-runners \
//! cargo test -p gridops-agent --test sandbox_live -- --ignored --nocapture
//! ```
//!
//! It downloads octocat/Hello-World, so it needs network access.

use std::time::Duration;

use agent_runtime::ToolCall;
use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use gridops_agent::{
    changes::{ChangeKind, changed_paths, collect_changes, prepare_checkout_script},
    sandbox::{ExecOutput, ExecRequest, Sandbox},
    tools::execute,
};
use serde_json::{Value, json};

struct Manager {
    http: reqwest::Client,
    base: reqwest::Url,
    token: String,
}

impl Manager {
    fn from_environment() -> Result<Self> {
        let base = std::env::var("GRIDOPS_SMOKE_MANAGER_URL")
            .context("GRIDOPS_SMOKE_MANAGER_URL is required")?;
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_mins(10))
                .build()?,
            base: reqwest::Url::parse(&base)?,
            token: std::env::var("GRIDOPS_SMOKE_MANAGER_TOKEN")
                .context("GRIDOPS_SMOKE_MANAGER_TOKEN is required")?,
        })
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut request = self
            .http
            .request(method, self.base.join(path)?)
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!("{path} failed ({status}): {text}");
        }
        Ok(if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)?
        })
    }
}

struct LiveSandbox<'a> {
    manager: &'a Manager,
    id: String,
}

#[async_trait]
impl Sandbox for LiveSandbox<'_> {
    async fn exec(&self, request: ExecRequest) -> Result<ExecOutput> {
        let value = self
            .manager
            .send(
                reqwest::Method::POST,
                &format!("v1/sandboxes/{}/exec", self.id),
                Some(json!({
                    "command": request.command,
                    "workdir": request.workdir,
                    "timeoutSeconds": request.timeout_seconds,
                    "maxOutputBytes": request.max_output_bytes,
                    "env": request.env,
                })),
            )
            .await?;
        Ok(ExecOutput {
            exit_code: value["exitCode"].as_i64().unwrap_or(-1),
            stdout: value["stdout"].as_str().unwrap_or_default().to_owned(),
            stderr: value["stderr"].as_str().unwrap_or_default().to_owned(),
            stdout_truncated: value["stdoutTruncated"].as_bool().unwrap_or(false),
            stderr_truncated: value["stderrTruncated"].as_bool().unwrap_or(false),
            timed_out: value["timedOut"].as_bool().unwrap_or(false),
            duration_ms: value["durationMs"].as_u64().unwrap_or(0),
        })
    }

    async fn upload_tar(&self, destination: &str, archive: Vec<u8>) -> Result<()> {
        let mut url = self
            .manager
            .base
            .join(&format!("v1/sandboxes/{}/archive", self.id))?;
        url.query_pairs_mut().append_pair("path", destination);
        let response = self
            .manager
            .http
            .put(url)
            .bearer_auth(&self.manager.token)
            .header("Content-Type", "application/x-tar")
            .body(archive)
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("upload failed: {}", response.text().await?);
        }
        Ok(())
    }
}

fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: name.into(),
        name: name.into(),
        arguments,
    }
}

#[tokio::test]
#[ignore = "needs a running runner manager and network access"]
async fn the_agent_tools_work_in_a_real_sandbox() -> Result<()> {
    let manager = Manager::from_environment()?;
    let network =
        std::env::var("GRIDOPS_SMOKE_NETWORK").unwrap_or_else(|_| "gridops-runners".into());
    let image = std::env::var("GRIDOPS_SMOKE_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/actions/actions-runner:latest".into());
    let id = uuid::Uuid::new_v4().to_string();
    let lease = manager
        .send(
            reqwest::Method::POST,
            "v1/admissions",
            Some(json!({
                "runnerId": id, "poolId": "gridops-agent", "provider": "docker",
                "cpuLimit": 1.0, "memoryLimitMb": 1024,
            })),
        )
        .await?;
    manager
        .send(
            reqwest::Method::POST,
            "v1/sandboxes",
            Some(json!({
                "sandboxId": id, "image": image, "pullImage": true, "cpuLimit": 1.0,
                "memoryLimitMb": 1024, "network": network,
                "capacityLease": lease["leaseId"],
            })),
        )
        .await?;
    let sandbox = LiveSandbox {
        manager: &manager,
        id: id.clone(),
    };
    let result = exercise(&sandbox).await;
    manager
        .send(reqwest::Method::DELETE, &format!("v1/sandboxes/{id}"), None)
        .await?;
    result
}

async fn exercise(sandbox: &LiveSandbox<'_>) -> Result<()> {
    let archive =
        reqwest::get("https://codeload.github.com/octocat/Hello-World/tar.gz/refs/heads/master")
            .await?
            .error_for_status()?
            .bytes()
            .await?;
    sandbox.upload_tar("/workspace", archive.to_vec()).await?;
    let baseline = sandbox
        .exec(ExecRequest {
            command: vec![
                "bash".into(),
                "-lc".into(),
                prepare_checkout_script().into(),
            ],
            workdir: "/workspace".into(),
            timeout_seconds: 600,
            max_output_bytes: 64 * 1_024,
            env: std::collections::BTreeMap::default(),
        })
        .await?
        .require_success("Preparing the checkout")?;
    println!("baseline: {}", baseline.trim());

    let listing = execute(sandbox, &call("list_files", json!({}))).await;
    assert!(!listing.is_error, "{}", listing.text);
    assert!(listing.text.contains("README"), "{}", listing.text);

    let read = execute(sandbox, &call("read_file", json!({ "path": "README" }))).await;
    assert!(read.text.contains("Hello World"), "{}", read.text);

    let found = execute(sandbox, &call("search", json!({ "pattern": "Hello" }))).await;
    assert!(found.text.contains("README:1:"), "{}", found.text);

    // An executable file committed into the baseline keeps its mode when edited.
    let setup = execute(
        sandbox,
        &call(
            "run_command",
            json!({ "command": "printf '#!/bin/sh\\necho one\\n' > run.sh && chmod 755 run.sh && git add run.sh && git commit -qm exec && echo ok" }),
        ),
    )
    .await;
    assert!(setup.text.contains("exit code: 0"), "{}", setup.text);

    for (name, arguments) in [
        (
            "edit_file",
            json!({ "path": "README", "old_text": "Hello World!", "new_text": "Hello GridOps!" }),
        ),
        (
            "edit_file",
            json!({ "path": "run.sh", "old_text": "echo one", "new_text": "echo two" }),
        ),
        (
            "write_file",
            json!({ "path": "scripts/check.sh", "content": "#!/bin/sh\nexit 0\n" }),
        ),
    ] {
        let result = execute(sandbox, &call(name, arguments)).await;
        assert!(!result.is_error, "{name}: {}", result.text);
    }
    let mode = execute(
        sandbox,
        &call(
            "run_command",
            json!({ "command": "stat -c %a run.sh && ./run.sh" }),
        ),
    )
    .await;
    assert!(
        mode.text.contains("755") && mode.text.contains("two"),
        "{}",
        mode.text
    );

    let escape = execute(
        sandbox,
        &call("read_file", json!({ "path": "../../etc/passwd" })),
    )
    .await;
    assert!(escape.is_error);

    let paths = changed_paths(sandbox).await?;
    let summary = paths
        .iter()
        .map(|change| format!("{:?} {} {}", change.kind, change.mode, change.path))
        .collect::<Vec<_>>();
    println!("changes: {summary:?}");
    assert_eq!(paths.len(), 3, "{summary:?}");
    let changes = collect_changes(sandbox).await?;
    let readme = changes
        .iter()
        .find(|change| change.path == "README")
        .context("README changed")?;
    assert_eq!(readme.kind, ChangeKind::Modified);
    assert!(
        String::from_utf8_lossy(readme.contents.as_deref().unwrap_or_default())
            .contains("Hello GridOps!")
    );
    let script = changes
        .iter()
        .find(|change| change.path == "run.sh")
        .context("run.sh changed")?;
    assert_eq!(script.mode, "100755");
    let added = changes
        .iter()
        .find(|change| change.path == "scripts/check.sh")
        .context("new file")?;
    assert_eq!(added.kind, ChangeKind::Added);
    assert_eq!(added.contents.as_deref(), Some(&b"#!/bin/sh\nexit 0\n"[..]));
    Ok(())
}
