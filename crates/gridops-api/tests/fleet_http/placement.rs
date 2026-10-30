//! A runner's whole fleet life against the API binary and the real agent:
//! profile, enrollment, approval, activation, admission, prepare, start with
//! one-time registration material, live console output, inspection and
//! cleanup. Only the container runtime is a stand-in.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use gridops_core::{
    Vault,
    fleet::{
        FleetService, Principal, SubmitIntent, WorkloadKind,
        ids::{Generation, PlacementId, WorkloadId},
        placement,
        protocol::{
            agent::{AgentOperation, LocalResourceIdentity, StartRunner},
            inventory::ObservedBackend,
            outcomes::{FailureCode, SuccessEvidence},
            primitives::WireTimestamp,
        },
    },
};
use gridops_host_agent::{
    client::Client,
    dispatch::Trust,
    helper::{HelperRequest, HelperResponse, RunnerOutput, RuntimeHelper},
    journal::Journal,
    supervisor::{Stop, Supervisor},
};
use secrecy::ExposeSecret as _;
use tokio::{io::AsyncWriteExt as _, sync::oneshot};

use super::*;

/// Plays a Docker host whose runner exits as soon as it's inspected, unless
/// it's `alive`.
#[derive(Default)]
pub(super) struct FakeDocker {
    pub(super) alive: AtomicBool,
    bootstraps: Mutex<Vec<String>>,
    pub(super) operations: Mutex<Vec<&'static str>>,
    /// How long each start takes, in milliseconds.
    pub(super) start_millis: AtomicU64,
    /// Prepares never finish while this is set.
    pub(super) hold_prepare: AtomicBool,
    /// The started container's console, written by the test.
    console: Mutex<Option<tokio::io::DuplexStream>>,
}

impl RuntimeHelper for FakeDocker {
    async fn call(&self, request: HelperRequest) -> HelperResponse {
        let kind = request.operation.database_kind();
        if let Ok(mut operations) = self.operations.lock() {
            operations.push(kind);
        }
        if matches!(request.operation, AgentOperation::StartRunner(_)) {
            let millis = self.start_millis.load(Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(millis)).await;
        }
        if matches!(request.operation, AgentOperation::PrepareEnvironment(_))
            && self.hold_prepare.load(Ordering::SeqCst)
        {
            std::future::pending::<()>().await;
        }
        let container = || LocalResourceIdentity::Docker {
            object_id: gridops_core::fleet::ids::UpstreamText::new("fake-container")
                .unwrap_or_else(|_| unreachable!()),
        };
        let evidence = match (&request.operation, request.environment_id) {
            (AgentOperation::PrepareEnvironment(_), Some(prepared_environment_id)) => {
                SuccessEvidence::PreparedEnvironment {
                    prepared_environment_id,
                }
            }
            (AgentOperation::StartRunner(start), Some(_)) => {
                let Some(material) = &request.bootstrap else {
                    return HelperResponse::Failed {
                        code: FailureCode::Configuration,
                        retryable: false,
                    };
                };
                if let Ok(mut bootstraps) = self.bootstraps.lock() {
                    bootstraps.push(material.0.clone());
                }
                SuccessEvidence::RunnerStarted {
                    prepared_environment_id: start.prepared_environment_id,
                    execution_profile_id: start.execution_profile_id,
                    profile_revision: start.profile_revision,
                    local_identity: container(),
                }
            }
            (AgentOperation::Inspect { expected_resource }, _)
                if self.alive.load(Ordering::SeqCst) =>
            {
                SuccessEvidence::Inspected {
                    local_identity: Some(expected_resource.clone()),
                }
            }
            // The runner has already exited.
            (AgentOperation::Inspect { .. }, _) => {
                return HelperResponse::Failed {
                    code: FailureCode::ResourceUnavailable,
                    retryable: false,
                };
            }
            (AgentOperation::Cleanup { expected_resource }, _) => {
                let now = WireTimestamp::from_millis(gridops_core::now_millis())
                    .unwrap_or_else(|_| unreachable!());
                let reference =
                    || -> gridops_core::fleet::protocol::outcomes::DiagnosticReference {
                        serde_json::from_value(json!({"id": Uuid::new_v4(), "code": "runtime"}))
                            .unwrap_or_else(|_| unreachable!())
                    };
                SuccessEvidence::Cleaned {
                    local_identity: expected_resource.clone(),
                    local_absence: serde_json::from_value(json!({
                        "resource": expected_resource, "observed_at": now,
                        "evidence": reference(),
                    }))
                    .unwrap_or_else(|_| unreachable!()),
                    delayed_start_exclusion: serde_json::from_value(json!({
                        "resource": expected_resource, "excluded_at": now,
                        "evidence": reference(),
                    }))
                    .unwrap_or_else(|_| unreachable!()),
                }
            }
            _ => {
                return HelperResponse::Failed {
                    code: FailureCode::Unsupported,
                    retryable: false,
                };
            }
        };
        HelperResponse::Succeeded {
            evidence: Box::new(evidence),
        }
    }

    async fn output(&self, _resource: LocalResourceIdentity) -> Option<RunnerOutput> {
        let console = self.console.lock().ok()?.take()?;
        Some(Box::pin(console))
    }

    async fn backends(&self) -> Vec<ObservedBackend> {
        serde_json::from_value(json!([{
            "local_key":"docker","known_id":null,"domain_local_key":"root",
            "runtime_kind":"docker","execution_os":"linux","architecture":"arm64",
            "readiness":"ready","reason":null,"runtime_identity":"docker-fake",
            "capabilities":{
                "schema_version":1,"runtime_kind":"docker","execution_os":"linux","architecture":"arm64",
                "supported_runners":[{"target_kind":"github_repository","mode":"ephemeral"}],
                "environments":[{"reference":"runner:fixture","readiness":"ready","version":null}],
                "resources":{"cpu":"hard_limit","memory":"hard_limit","disk":"estimate"},
                "interactive":"unsupported","docker_socket":"unsupported"
            }
        }]))
        .unwrap_or_default()
    }
}

pub(super) async fn seed_pool(f: &Fixture) -> Result<()> {
    sqlx::raw_sql(
        "INSERT INTO installations (id,account_id,account_login,account_type,target_type,repository_selection,created_at,updated_at)
           VALUES (1,1,'octo-org','Organization','Organization','selected',1,1);
         INSERT INTO repositories (id,installation_id,owner,name,full_name,private,default_branch,html_url,last_synced_at,created_at,updated_at)
           VALUES (10,1,'octo-org','gridops','octo-org/gridops',0,'master','https://github.com/octo-org/gridops',1,1,1);
         INSERT INTO runner_pools (id,installation_id,repository_id,name,scope,provider,providers,labels,image,docker_image,tart_image,created_at,updated_at)
           VALUES ('fleet-pool',1,10,'fleet','repository','docker','[\"docker\"]','[\"self-hosted\"]','runner:fixture','runner:fixture','macos',1,1);
         INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at) VALUES ('fleet-pool',10,1);
         INSERT INTO user_installations (user_id,installation_id,permission,created_at) VALUES ('fleet-user',1,'admin',1);",
    )
    .execute(&f.database)
    .await?;
    Ok(())
}

pub(super) async fn eventually<T, F, Fut>(what: &str, mut probe: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Option<T>>>,
{
    for _ in 0..200 {
        if let Some(value) = probe().await? {
            return Ok(value);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    bail!("timed out waiting for {what}")
}

pub(super) async fn placement_state(f: &Fixture, placement: PlacementId) -> Result<String> {
    Ok(
        sqlx::query_scalar("SELECT state FROM workload_placements WHERE id=?")
            .bind(placement.to_string())
            .fetch_one(&f.database)
            .await?,
    )
}

/// A machine that enrolled through `origin` (the API, or a proxy in front of
/// it), was approved, had its Docker backend enabled and was activated, and
/// now holds work authority.
pub(super) struct Machine {
    pub(super) host: gridops_core::fleet::ids::HostId,
    pub(super) journal: Journal,
    pub(super) target: String,
    pub(super) profile_id: String,
    pub(super) stop: oneshot::Sender<()>,
    pub(super) running: tokio::task::JoinHandle<Result<Stop>>,
}

pub(super) async fn active_machine(
    f: &Fixture,
    origin: &str,
    runtime: Arc<FakeDocker>,
) -> Result<Machine> {
    active_machine_on(
        f,
        origin,
        runtime,
        json!([{"runtimeKind": "docker", "executionOs": "linux", "architecture": "arm64", "image": "runner:fixture"}]),
    )
    .await
}

/// Like [`active_machine`], with the pool running on `platforms`. The
/// machine itself offers Linux arm64 Docker.
pub(super) async fn active_machine_on(
    f: &Fixture,
    origin: &str,
    runtime: Arc<FakeDocker>,
    platforms: Value,
) -> Result<Machine> {
    let response = f
        .client
        .put(f.url("/pools/fleet-pool/profile"))
        .header(header::COOKIE, &f.cookie)
        .header(header::ORIGIN, &f.origin)
        .json(&json!({
            "enabled": true, "mode": "ephemeral", "cpuMillis": 1000, "memoryMib": 1024, "diskBytes": 0,
            "platforms": platforms,
        }))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let profile: Value = response.json().await?;
    let target = profile["item"]["targetIds"][0]
        .as_str()
        .context("profile target")?
        .to_owned();

    let issued: Value = f
        .admin_post("/enrollments")
        // One code per machine, so a test can enroll several.
        .header(
            "idempotency-key",
            format!("placement-enrollment-{}", Uuid::new_v4()),
        )
        .json(&json!({"targetIds": [target]}))
        .send()
        .await?
        .json()
        .await?;
    let journal =
        Journal::open(&f.directory.join(format!("agent-{}.sqlite", Uuid::new_v4()))).await?;
    let supervisor = supervisor(origin, journal.clone(), Arc::clone(&runtime))?;
    supervisor
        .enroll(issued["code"].as_str().context("code")?, None)
        .await?;
    let host = journal.identity().await?.context("identity")?.host_id;
    let (stop, running) = run(supervisor);

    let read = eventually("inventory", || async {
        let read: Value = f
            .client
            .get(f.url(&format!("/hosts/{host}")))
            .header(header::COOKIE, &f.cookie)
            .send()
            .await?
            .json()
            .await?;
        Ok((read["item"]["approvalEvidence"]["state"] == "available").then_some(read))
    })
    .await?;
    let evidence = &read["item"]["approvalEvidence"];
    let approved = f
        .admin_post(&format!("/hosts/{host}/approve"))
        .json(&json!({
            "expectedRevision": evidence["expectedRevision"],
            "expectedInventoryRevision": evidence["expectedInventoryRevision"],
            "expectedInventoryDigest": evidence["expectedInventoryDigest"],
            "targetIds": [target],
        }))
        .send()
        .await?;
    assert_eq!(approved.status(), StatusCode::OK);
    let backend: String = sqlx::query_scalar("SELECT id FROM host_backends WHERE host_id=?")
        .bind(host.to_string())
        .fetch_one(&f.database)
        .await?;
    let enabled = f
        .admin_post(&format!("/hosts/{host}/backends/{backend}/enabled"))
        .json(&json!({"enabled": true}))
        .send()
        .await?;
    assert_eq!(enabled.status(), StatusCode::OK);
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM fleet_hosts WHERE id=?")
        .bind(host.to_string())
        .fetch_one(&f.database)
        .await?;
    let activated = f
        .admin_post(&format!("/hosts/{host}/lifecycle"))
        .json(&json!({"expectedRevision": revision, "lifecycle": "active"}))
        .send()
        .await?;
    assert_eq!(activated.status(), StatusCode::OK);
    eventually("work authority", || async {
        let expires: Option<i64> =
            sqlx::query_scalar("SELECT authority_expires_at FROM fleet_hosts WHERE id=?")
                .bind(host.to_string())
                .fetch_one(&f.database)
                .await?;
        Ok(expires.map(|_| ()))
    })
    .await?;

    // The platform the machine serves.
    let profile_id = profile["item"]["platforms"]
        .as_array()
        .context("platforms")?
        .iter()
        .find(|platform| platform["architecture"] == "arm64")
        .and_then(|platform| platform["id"].as_str())
        .context("profile id")?
        .to_owned();
    Ok(Machine {
        host,
        journal,
        target,
        profile_id,
        stop,
        running,
    })
}

/// The machine's agent, with its journal in `journal`.
pub(super) fn supervisor(
    origin: &str,
    journal: Journal,
    runtime: Arc<FakeDocker>,
) -> Result<Supervisor<Arc<FakeDocker>>> {
    Ok(
        Supervisor::new(Client::new(origin)?, journal, "fleet-host".into())
            .with_heartbeat(Duration::from_millis(200))
            .with_measurements(|| gridops_host_agent::probe::Measurement {
                memory_total_mib: Some(16_384),
                memory_used_mib: Some(4_096),
                cpu_used_millis: Some(500),
                uptime_seconds: Some(60),
            })
            .with_runtime(runtime, Trust::default()),
    )
}

/// Runs an enrolled agent until the returned sender fires.
pub(super) fn run(
    supervisor: Supervisor<Arc<FakeDocker>>,
) -> (oneshot::Sender<()>, tokio::task::JoinHandle<Result<Stop>>) {
    let (stop, stopped) = oneshot::channel::<()>();
    let running = tokio::spawn(async move {
        supervisor
            .run(async {
                let _ = stopped.await;
            })
            .await
    });
    (stop, running)
}

/// Admits one more runner of the pool onto `machine` and waits until its
/// environment is prepared.
pub(super) async fn prepared_placement(
    f: &Fixture,
    machine: &Machine,
    workload: Uuid,
) -> Result<PlacementId> {
    let placement_id = admitted_placement(f, machine, workload).await?;
    eventually("prepared environment", || async {
        Ok((placement_state(f, placement_id).await? == "registering").then_some(()))
    })
    .await?;
    Ok(placement_id)
}

/// Admits one more runner of the pool onto `machine`, which is sent its
/// prepare.
pub(super) async fn admitted_placement(
    f: &Fixture,
    machine: &Machine,
    workload: Uuid,
) -> Result<PlacementId> {
    // As the reconciler does: the runner row exists before placement, and
    // must not count against a pool that allows exactly the runners it has.
    sqlx::query(
        "INSERT INTO runners (id,pool_id,name,provider,ci_platform,os,architecture,status,ephemeral,configuration_version,created_at,updated_at)
         VALUES (?,'fleet-pool','fleet-runner','docker','github','linux','ARM64','starting',1,1,1,1)",
    )
    .bind(workload.to_string())
    .execute(&f.database)
    .await?;
    sqlx::query(
        "UPDATE runner_pools SET max_count=(SELECT COUNT(*) FROM runners WHERE pool_id='fleet-pool')
          WHERE id='fleet-pool'",
    )
    .execute(&f.database)
    .await?;
    let profile_id = machine.profile_id.as_str();
    // A pool set to "machines first" would place its next runner here.
    assert!(
        gridops_core::fleet::targets::machine_ready(&f.database, "fleet-pool", profile_id).await?
    );
    let service = FleetService::new(f.database.clone());
    service
        .submit_intent(
            &Principal::Reconciler,
            SubmitIntent {
                idempotency_key: format!("runner:{workload}"),
                pool_id: "fleet-pool".into(),
                profile_id: profile_id.parse()?,
                target_id: machine.target.parse()?,
                workload_id: WorkloadId::try_new(workload)?,
                generation: Generation::new(1)?,
                kind: WorkloadKind::CiRunner,
                agent_run_id: None,
                expected_profile_revision: Some(1),
            },
        )
        .await?;
    let report = service.admit_queued(&Principal::Reconciler, 10).await?;
    let reason: Option<String> =
        sqlx::query_scalar("SELECT reason_code FROM workload_intents WHERE workload_id=?")
            .bind(workload.to_string())
            .fetch_one(&f.database)
            .await?;
    assert_eq!(report.admitted, 1, "not admitted: {reason:?}");
    let placement_id: PlacementId =
        sqlx::query_scalar::<_, String>("SELECT id FROM workload_placements WHERE workload_id=?")
            .bind(workload.to_string())
            .fetch_one(&f.database)
            .await?
            .parse()?;
    Ok(placement_id)
}

/// Issues the placement's start, with one-time registration material, that
/// the host must act on within `ttl_ms`.
pub(super) async fn issue_start(
    f: &Fixture,
    placement_id: PlacementId,
    workload: Uuid,
    ttl_ms: i64,
) -> Result<()> {
    let prepared = placement::prepared_environment(&f.database, placement_id)
        .await?
        .context("prepared environment")?;
    let prepare = placement::prepared_with(&f.database, placement_id)
        .await?
        .context("prepare command")?;
    let vault = Vault::new([0; 32], SESSION_SECRET.as_bytes().to_vec());
    let reference = placement::store_bootstrap(
        &f.database,
        &vault,
        placement_id,
        r#"{"kind":"github_jit","jit_config":"one-time-jit"}"#,
        60_000,
    )
    .await?;
    let start = AgentOperation::StartRunner(StartRunner {
        prepared_environment_id: prepared,
        execution_profile_id: prepare.execution_profile_id,
        profile_revision: prepare.profile_revision,
        bootstrap_reference: reference,
        bootstrap_expires_at: WireTimestamp::from_millis(gridops_core::now_millis() + 60_000)?,
        resource_request: prepare.resource_request,
        allocation_ids: prepare.allocation_ids,
    });
    placement::mark_starting(&f.database, placement_id, &workload.to_string()).await?;
    placement::issue_command(
        &f.database,
        placement_id,
        start,
        &format!("start:{placement_id}"),
        ttl_ms,
    )
    .await?
    .context("start command")?;

    Ok(())
}

#[tokio::test]
async fn admitted_runner_is_prepared_started_streamed_and_cleaned_on_its_host() -> Result<()> {
    let f = Fixture::new().await?;
    seed_pool(&f).await?;
    let runtime = Arc::new(FakeDocker::default());
    let (mut console, output) = tokio::io::duplex(64 * 1024);
    *runtime
        .console
        .lock()
        .map_err(|_| anyhow::anyhow!("poisoned"))? = Some(output);
    let machine = active_machine(&f, &f.origin, Arc::clone(&runtime)).await?;
    let workload = Uuid::new_v4();
    let placement_id = prepared_placement(&f, &machine, workload).await?;
    issue_start(&f, placement_id, workload, 55_000).await?;
    let Machine {
        host,
        journal,
        stop,
        running,
        ..
    } = machine;
    let vault = Vault::new([0; 32], SESSION_SECRET.as_bytes().to_vec());
    eventually("running runner", || async {
        Ok((placement_state(&f, placement_id).await? == "running").then_some(()))
    })
    .await?;
    assert!(
        runtime
            .bootstraps
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned"))?
            .iter()
            .any(|material| material.contains("one-time-jit"))
    );
    let erased: Option<String> =
        sqlx::query_scalar("SELECT sealed FROM fleet_bootstrap_material WHERE placement_id=?")
            .bind(placement_id.to_string())
            .fetch_one(&f.database)
            .await?;
    assert!(erased.is_none(), "registration material outlived its start");

    // The runner picks up a job; its console output shows in the job log
    // view while the job runs.
    let now = gridops_core::now_millis();
    sqlx::query("UPDATE users SET access_token=? WHERE id='fleet-user'")
        .bind(vault.seal("fixture-token")?)
        .execute(&f.database)
        .await?;
    sqlx::raw_sql(
        "INSERT INTO workflow_runs (id,repository_id,workflow_name,run_number,event,status,head_sha,html_url,github_created_at,github_updated_at,created_at,updated_at)
           VALUES (70,10,'CI',7,'push','in_progress','abc','https://github.com/octo-org/gridops/actions/runs/70',1,1,1,1);
         INSERT INTO workflow_jobs (id,run_id,name,status,html_url,started_at,created_at,updated_at)
           VALUES (700,70,'test','in_progress','https://github.com/octo-org/gridops/actions/runs/70/job/700',1,1,1);",
    )
    .execute(&f.database)
    .await?;
    sqlx::query("UPDATE runners SET current_job_id=700 WHERE id=?")
        .bind(workload.to_string())
        .execute(&f.database)
        .await?;
    let job_view = || async {
        let view: Value = f
            .client
            .get(format!("{}/api/v1/workflow-jobs/700/logs", f.origin))
            .header(header::COOKIE, &f.cookie)
            .send()
            .await?
            .json()
            .await?;
        anyhow::Ok(view)
    };
    let first = "2026-10-14T10:00:00.000000000Z [GRIDOPS JOB LOG 1.log]\n\
        2026-10-14T10:00:01.000000000Z 2026-10-14T10:00:01.0000000Z ##[group]Run cargo test\n\
        2026-10-14T10:00:02.000000000Z 2026-10-14T10:00:02.0000000Z    Compiling gridops-core v0.1.0\n";
    console.write_all(first.as_bytes()).await?;
    let view = eventually("live runner output", || async {
        let view = job_view().await?;
        Ok(view["steps"]
            .to_string()
            .contains("Compiling gridops-core")
            .then_some(view))
    })
    .await?;
    assert_eq!(view["source"], "runner");
    assert_eq!(view["status"], "in_progress");
    let stream = || async {
        anyhow::Ok(
            sqlx::query_as::<_, (i64, i64, bool, Option<i64>)>(
                "SELECT o.received_bytes,s.size_bytes,s.complete,s.job_id FROM fleet_output_streams o
                   JOIN log_streams s ON s.id=o.log_stream_id WHERE o.placement_id=?",
            )
            .bind(placement_id.to_string())
            .fetch_one(&f.database)
            .await?,
        )
    };
    let shipped = i64::try_from(first.len())?;
    eventually("shipped output", || async {
        Ok((stream().await? == (shipped, shipped, false, None)).then_some(()))
    })
    .await?;

    // A replayed chunk is appended once, and only the placement's own host,
    // in its live session, can write the placement's output.
    let identity = journal.identity().await?.context("identity")?;
    let credential = identity.credential.expose_secret().to_owned();
    let session: String = sqlx::query_scalar("SELECT agent_session_id FROM fleet_hosts WHERE id=?")
        .bind(host.to_string())
        .fetch_one(&f.database)
        .await?;
    let chunk = |session: &str, data: &str| {
        json!({
            "authority_session_id": session, "placement_id": placement_id,
            "offset": 0, "data": base64::engine::general_purpose::STANDARD.encode(data),
        })
    };
    let replay: Value = inventory::agent_post(&f, &host.to_string(), &credential, "output")
        .json(&chunk(&session, first))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(replay, json!({"received": shipped, "finished": false}));
    error(
        inventory::agent_post(&f, &host.to_string(), &credential, "output")
            .json(&chunk(&Uuid::new_v4().to_string(), "forged\n"))
            .send()
            .await?,
        StatusCode::FORBIDDEN,
        "forbidden",
    )
    .await?;
    let (other, other_credential) = inventory::enrolled(&f).await?;
    let other_session = Uuid::new_v4().to_string();
    sqlx::query("UPDATE fleet_hosts SET agent_session_id=?,authority_expires_at=? WHERE id=?")
        .bind(&other_session)
        .bind(now + 60_000)
        .bind(&other)
        .execute(&f.database)
        .await?;
    error(
        inventory::agent_post(&f, &other, &other_credential, "output")
            .json(&chunk(&other_session, "forged\n"))
            .send()
            .await?,
        StatusCode::NOT_FOUND,
        "not_found",
    )
    .await?;
    assert_eq!(stream().await?, (shipped, shipped, false, None));

    // When the runner stops, the rest arrives and the stream is kept as the
    // job's runner log.
    let last =
        "2026-10-14T10:00:03.000000000Z 2026-10-14T10:00:03.0000000Z test result: ok. 12 passed\n";
    console.write_all(last.as_bytes()).await?;
    drop(console);
    let total = shipped + i64::try_from(last.len())?;
    eventually("finished output", || async {
        Ok((stream().await? == (total, total, true, Some(700))).then_some(()))
    })
    .await?;
    assert!(
        job_view().await?["steps"]
            .to_string()
            .contains("test result: ok")
    );

    let container = LocalResourceIdentity::Docker {
        object_id: gridops_core::fleet::ids::UpstreamText::new("fake-container")?,
    };
    placement::issue_command(
        &f.database,
        placement_id,
        AgentOperation::Inspect {
            expected_resource: container.clone(),
        },
        "inspect:test",
        25_000,
    )
    .await?
    .context("inspect command")?;
    eventually("exit observed", || async {
        Ok(
            placement::reported_gone(&f.database, placement_id, "inspect")
                .await?
                .then_some(()),
        )
    })
    .await?;
    placement::issue_command(
        &f.database,
        placement_id,
        AgentOperation::Cleanup {
            expected_resource: container,
        },
        "cleanup:test",
        55_000,
    )
    .await?
    .context("cleanup command")?;
    eventually("cleaned placement", || async {
        Ok((placement_state(&f, placement_id).await? == "cleaned").then_some(()))
    })
    .await?;
    let held: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM capacity_allocations WHERE placement_id=? AND state!='released'",
    )
    .bind(placement_id.to_string())
    .fetch_one(&f.database)
    .await?;
    assert_eq!(held, 0, "capacity was not released");
    // A placement that was told to start is never released by abandonment;
    // only cleanup can prove its runner gone.
    assert!(!placement::abandon_unstarted(&f.database, placement_id).await?);
    assert_eq!(
        *runtime
            .operations
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned"))?,
        ["prepare_environment", "start_runner", "inspect", "cleanup"]
    );

    let _ = stop.send(());
    assert_eq!(running.await??, Stop::Shutdown);
    Ok(())
}

#[tokio::test]
async fn saving_a_fleet_profile_is_recorded_in_the_audit_trail() -> Result<()> {
    let f = Fixture::new().await?;
    seed_pool(&f).await?;
    let linux = json!({"runtimeKind": "docker", "executionOs": "linux", "architecture": "arm64"});
    let response = f
        .client
        .put(f.url("/pools/fleet-pool/profile"))
        .header(header::COOKIE, &f.cookie)
        .header(header::ORIGIN, &f.origin)
        .json(&json!({
            "enabled": true, "mode": "ephemeral", "cpuMillis": 1000, "memoryMib": 1024,
            "diskBytes": 0, "platforms": [linux],
        }))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action='fleet.pool.profile_saved' AND target_id='fleet-pool'",
    )
    .fetch_one(&f.database)
    .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[tokio::test]
async fn pool_runs_on_several_platforms_and_keeps_unchanged_ones() -> Result<()> {
    let f = Fixture::new().await?;
    seed_pool(&f).await?;
    let save = |platforms: Value| {
        f.client
            .put(f.url("/pools/fleet-pool/profile"))
            .header(header::COOKIE, &f.cookie)
            .header(header::ORIGIN, &f.origin)
            .json(&json!({
                "enabled": true, "mode": "ephemeral", "cpuMillis": 1000, "memoryMib": 1024,
                "diskBytes": 0, "platforms": platforms,
            }))
            .send()
    };
    // Without an image, a Docker platform runs the pool's own Docker image.
    let linux = json!({"runtimeKind": "docker", "executionOs": "linux", "architecture": "arm64"});
    let windows = json!({"runtimeKind": "native_process", "executionOs": "windows", "architecture": "x64", "image": "native:actions-runner"});
    let saved: Value = save(json!([linux, windows])).await?.json().await?;
    let platforms = saved["item"]["platforms"].as_array().context("platforms")?;
    assert_eq!(platforms.len(), 2);
    assert_eq!(platforms[0]["image"], "runner:fixture");
    let (linux_id, windows_id) = (platforms[0]["id"].clone(), platforms[1]["id"].clone());

    // The same platform twice is refused.
    assert_eq!(
        save(json!([linux, linux])).await?.status(),
        StatusCode::BAD_REQUEST
    );

    // Dropping Windows archives it; Linux keeps its ID and revision.
    let mut kept = linux.clone();
    kept["id"] = linux_id.clone();
    let saved: Value = save(json!([kept])).await?.json().await?;
    assert_eq!(saved["item"]["platforms"].as_array().map(Vec::len), Some(1));
    assert_eq!(saved["item"]["platforms"][0]["id"], linux_id);
    assert_eq!(saved["item"]["platforms"][0]["revision"], 1);
    let archived: (bool, Option<i64>) =
        sqlx::query_as("SELECT enabled,archived_at FROM pool_execution_profiles WHERE id=?")
            .bind(windows_id.as_str().context("windows id")?)
            .fetch_one(&f.database)
            .await?;
    assert!(!archived.0 && archived.1.is_some());

    // Matching labels: a Windows x64 job needs the Windows platform.
    let requested = [
        "self-hosted".to_owned(),
        "Windows".to_owned(),
        "X64".to_owned(),
    ];
    assert!(gridops_core::fleet::targets::platform_supports_labels(
        gridops_core::fleet::ExecutionOs::Windows,
        gridops_core::fleet::Architecture::X64,
        &requested,
        &[],
    ));
    assert!(!gridops_core::fleet::targets::platform_supports_labels(
        gridops_core::fleet::ExecutionOs::Linux,
        gridops_core::fleet::Architecture::Arm64,
        &requested,
        &[],
    ));
    Ok(())
}

#[tokio::test]
async fn waiting_runners_explain_what_is_missing() -> Result<()> {
    let f = Fixture::new().await?;
    seed_pool(&f).await?;
    let profile: Value = f
        .client
        .put(f.url("/pools/fleet-pool/profile"))
        .header(header::COOKIE, &f.cookie)
        .header(header::ORIGIN, &f.origin)
        .json(&json!({
            "enabled": true, "mode": "ephemeral", "cpuMillis": 1000, "memoryMib": 1024, "diskBytes": 0,            "platforms": [{"runtimeKind": "docker", "executionOs": "linux", "architecture": "arm64", "image": "runner:fixture"}],
        }))
        .send()
        .await?
        .json()
        .await?;
    let service = FleetService::new(f.database.clone());
    service
        .submit_intent(
            &Principal::Reconciler,
            SubmitIntent {
                idempotency_key: "runner:waiting".into(),
                pool_id: "fleet-pool".into(),
                profile_id: profile["item"]["platforms"][0]["id"]
                    .as_str()
                    .context("profile")?
                    .parse()?,
                target_id: profile["item"]["targetIds"][0]
                    .as_str()
                    .context("target")?
                    .parse()?,
                workload_id: WorkloadId::try_new(Uuid::new_v4())?,
                generation: Generation::new(1)?,
                kind: WorkloadKind::CiRunner,
                agent_run_id: None,
                expected_profile_revision: Some(1),
            },
        )
        .await?;
    assert_eq!(
        service
            .admit_queued(&Principal::Reconciler, 10)
            .await?
            .admitted,
        0
    );
    let queue: Value = f
        .client
        .get(f.url("/pools/fleet-pool/queue"))
        .header(header::COOKIE, &f.cookie)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(queue["items"][0]["reason"], "no_active_machine");

    // A repository that joins the pool after its platforms were saved is
    // refused until the reconciler links it; one outside the pool never is.
    sqlx::raw_sql(
        "INSERT INTO repositories (id,installation_id,owner,name,full_name,private,default_branch,html_url,last_synced_at,created_at,updated_at)
           VALUES (11,1,'octo-org','new','octo-org/new',0,'main','https://github.com/octo-org/new',1,1,1),
                  (12,1,'octo-org','other','octo-org/other',0,'main','https://github.com/octo-org/other',1,1,1);
         INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at) VALUES ('fleet-pool',11,1);",
    )
    .execute(&f.database)
    .await?;
    let target = |repository: u64| {
        let database = f.database.clone();
        async move {
            anyhow::Ok(
                gridops_core::fleet::targets::ensure_target(
                    &database,
                    &gridops_core::fleet::CiTarget::GithubRepository {
                        installation_id: 1.try_into()?,
                        repository_id: repository.try_into()?,
                    },
                )
                .await?,
            )
        }
    };
    let submit = |target_id: String| {
        let service = service.clone();
        let profile_id = profile["item"]["platforms"][0]["id"].clone();
        async move {
            service
                .submit_intent(
                    &Principal::Reconciler,
                    SubmitIntent {
                        idempotency_key: format!("runner:{target_id}"),
                        pool_id: "fleet-pool".into(),
                        profile_id: profile_id.as_str().context("profile")?.parse()?,
                        target_id: target_id.parse()?,
                        workload_id: WorkloadId::try_new(Uuid::new_v4())?,
                        generation: Generation::new(1)?,
                        kind: WorkloadKind::CiRunner,
                        agent_run_id: None,
                        expected_profile_revision: Some(1),
                    },
                )
                .await
                .map_err(anyhow::Error::from)
        }
    };
    let (joined, outside) = (target(11).await?, target(12).await?);
    assert!(submit(joined.clone()).await.is_err());
    for target_id in [&joined, &outside] {
        gridops_core::fleet::targets::link_pool_target(&f.database, "fleet-pool", target_id)
            .await?;
    }
    submit(joined).await?;
    assert!(submit(outside).await.is_err());
    // With no machine able to take it, "machines first" uses the built-in host.
    assert!(
        !gridops_core::fleet::targets::machine_ready(
            &f.database,
            "fleet-pool",
            profile["item"]["platforms"][0]["id"]
                .as_str()
                .context("profile")?
        )
        .await?
    );

    // A pool an operator paused says so rather than blaming the machines.
    let machine = active_machine(&f, &f.origin, Arc::new(FakeDocker::default())).await?;
    let runner = Uuid::new_v4();
    service
        .submit_intent(
            &Principal::Reconciler,
            SubmitIntent {
                idempotency_key: "runner:ready".into(),
                pool_id: "fleet-pool".into(),
                profile_id: machine.profile_id.parse()?,
                target_id: machine.target.parse()?,
                workload_id: WorkloadId::try_new(runner)?,
                generation: Generation::new(1)?,
                kind: WorkloadKind::CiRunner,
                agent_run_id: None,
                expected_profile_revision: None,
            },
        )
        .await?;
    let admit_in = |state: &'static str| {
        let (database, service) = (f.database.clone(), service.clone());
        async move {
            sqlx::query("UPDATE runner_pools SET paused=?,state=? WHERE id='fleet-pool'")
                .bind(state == "draining")
                .bind(state)
                .execute(&database)
                .await?;
            anyhow::Ok(
                service
                    .admit_queued(&Principal::Reconciler, 10)
                    .await?
                    .admitted,
            )
        }
    };
    assert_eq!(admit_in("draining").await?, 0);
    let queue: Value = f
        .client
        .get(f.url("/pools/fleet-pool/queue"))
        .header(header::COOKIE, &f.cookie)
        .send()
        .await?
        .json()
        .await?;
    let waiting = queue["items"]
        .as_array()
        .context("queue")?
        .iter()
        .find(|item| item["runnerId"] == runner.to_string())
        .context("queued runner")?;
    assert_eq!(waiting["reason"], "pool_paused");
    // The reconcile pass that raises a pool to scaling is the one queuing the
    // runner, so a ready machine takes it straight away.
    assert_eq!(admit_in("scaling").await?, 1);
    Ok(())
}

#[tokio::test]
async fn pools_take_turns_when_work_waits() -> Result<()> {
    let f = Fixture::new().await?;
    seed_pool(&f).await?;
    sqlx::raw_sql(
        "INSERT INTO runner_pools (id,installation_id,repository_id,name,scope,provider,providers,labels,image,docker_image,tart_image,created_at,updated_at)
           VALUES ('other-pool',1,10,'other','repository','docker','[\"docker\"]','[]','runner:fixture','runner:fixture','macos',1,1);
         INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at) VALUES ('other-pool',10,1);",
    )
    .execute(&f.database)
    .await?;
    let mut profiles = Vec::new();
    for pool in ["fleet-pool", "other-pool"] {
        let profile: Value = f
            .client
            .put(f.url(&format!("/pools/{pool}/profile")))
            .header(header::COOKIE, &f.cookie)
            .header(header::ORIGIN, &f.origin)
            .json(&json!({
                "enabled": true, "mode": "ephemeral", "cpuMillis": 1000, "memoryMib": 1024, "diskBytes": 0,                "platforms": [{"runtimeKind": "docker", "executionOs": "linux", "architecture": "arm64", "image": "runner:fixture"}],
            }))
            .send()
            .await?
            .json()
            .await?;
        profiles.push((pool, profile));
    }
    let service = FleetService::new(f.database.clone());
    // The busy pool queues three runners before the other pool queues one.
    for (index, (pool, profile)) in [&profiles[0], &profiles[0], &profiles[0], &profiles[1]]
        .into_iter()
        .enumerate()
    {
        service
            .submit_intent(
                &Principal::Reconciler,
                SubmitIntent {
                    idempotency_key: format!("runner:turn-{index}"),
                    pool_id: (*pool).into(),
                    profile_id: profile["item"]["platforms"][0]["id"]
                        .as_str()
                        .context("profile")?
                        .parse()?,
                    target_id: profile["item"]["targetIds"][0]
                        .as_str()
                        .context("target")?
                        .parse()?,
                    workload_id: WorkloadId::try_new(Uuid::new_v4())?,
                    generation: Generation::new(1)?,
                    kind: WorkloadKind::CiRunner,
                    agent_run_id: None,
                    expected_profile_revision: Some(1),
                },
            )
            .await?;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Two admission attempts: one per pool, not the busy pool's first two.
    service.admit_queued(&Principal::Reconciler, 2).await?;
    let considered: Vec<String> = sqlx::query_scalar(
        "SELECT pool_id FROM workload_intents WHERE reason_code IS NOT NULL ORDER BY pool_id",
    )
    .fetch_all(&f.database)
    .await?;
    assert_eq!(considered, ["fleet-pool", "other-pool"]);
    Ok(())
}

#[tokio::test]
async fn prioritised_repositories_get_the_next_runner_first() -> Result<()> {
    let f = Fixture::new().await?;
    seed_pool(&f).await?;
    let now = gridops_core::now_millis();
    // Repository 10 queued two jobs before repository 11 queued one.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO repositories (id,installation_id,owner,name,full_name,private,default_branch,html_url,last_synced_at,created_at,updated_at)
           VALUES (11,1,'octo-org','release','octo-org/release',0,'main','https://github.com/octo-org/release',1,1,1);
         INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at) VALUES ('fleet-pool',11,2);
         UPDATE runner_pools SET autoscaling_enabled=1,paused=0 WHERE id='fleet-pool';
         INSERT INTO workflow_runs (id,repository_id,workflow_name,run_number,event,status,head_sha,html_url,github_created_at,github_updated_at,created_at,updated_at)
           VALUES (40,10,'CI',1,'push','queued','sha','https://example.test/40',1,1,1,1),
                  (41,11,'Release',1,'push','queued','sha','https://example.test/41',1,1,1,1);
         INSERT INTO workflow_jobs (id,run_id,name,status,labels,html_url,created_at,updated_at)
           VALUES (50,40,'build','queued','[\"self-hosted\"]','https://example.test/50',{older},{older}),
                  (51,40,'test','queued','[\"self-hosted\"]','https://example.test/51',{older},{older}),
                  (52,41,'ship','queued','[\"self-hosted\"]','https://example.test/52',{newer},{newer});",
        older = now - 60_000,
        newer = now - 30_000,
    )))
    .execute(&f.database)
    .await?;
    let next_repository = || async {
        Ok::<_, anyhow::Error>(
            gridops_core::next_runner_repository(&f.database, "fleet-pool", 1)
                .await?
                .context("next repository")?
                .repository_id,
        )
    };
    // Unprioritised, the repository furthest behind gets the runner.
    assert_eq!(next_repository().await?, 10);

    let response = f
        .client
        .put(format!("{}/api/v1/runner-pools/fleet-pool", f.origin))
        .header(header::COOKIE, &f.cookie)
        .header(header::ORIGIN, &f.origin)
        .json(&json!({
            "name": "fleet", "mode": "ephemeral", "provider": "docker", "providers": ["docker"],
            "labels": [], "image": "runner:fixture", "dockerImage": "runner:fixture",
            "desiredCount": 0, "minCount": 0, "maxCount": 4, "autoscalingEnabled": true,
            "queueScaleFactor": 1, "idleTimeoutMinutes": 5, "cpuLimit": 1, "memoryLimitMb": 1024,
            "runnerGroupId": 1, "priorityRepositoryIds": [11], "priorityWaitMinutes": 10,
        }))
        .send()
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        response.text().await?
    );
    let pool: Value = f
        .client
        .get(format!("{}/api/v1/runner-pools/fleet-pool", f.origin))
        .header(header::COOKIE, &f.cookie)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(pool["priorityRepositoryIds"], json!([11]));
    // The built-in host's next runner goes to the prioritised repository.
    assert_eq!(next_repository().await?, 11);

    // On machines, the prioritised repository's runner is admitted first,
    // though the other repository's were queued before it.
    let profile: Value = f
        .client
        .put(f.url("/pools/fleet-pool/profile"))
        .header(header::COOKIE, &f.cookie)
        .header(header::ORIGIN, &f.origin)
        .json(&json!({
            "enabled": true, "mode": "ephemeral", "cpuMillis": 1000, "memoryMib": 1024, "diskBytes": 0,
            "platforms": [{"runtimeKind": "docker", "executionOs": "linux", "architecture": "arm64", "image": "runner:fixture"}],
        }))
        .send()
        .await?
        .json()
        .await?;
    let profile_id = profile["item"]["platforms"][0]["id"]
        .as_str()
        .context("profile")?;
    let service = FleetService::new(f.database.clone());
    for (index, repository_id) in [10_u64, 10, 11].into_iter().enumerate() {
        let target = gridops_core::fleet::targets::ensure_target(
            &f.database,
            &gridops_core::fleet::CiTarget::GithubRepository {
                installation_id: gridops_core::fleet::ExternalId::new(1)?,
                repository_id: gridops_core::fleet::ExternalId::new(repository_id)?,
            },
        )
        .await?;
        gridops_core::fleet::targets::link_pool_target(&f.database, "fleet-pool", &target).await?;
        service
            .submit_intent(
                &Principal::Reconciler,
                SubmitIntent {
                    idempotency_key: format!("runner:priority-{index}"),
                    pool_id: "fleet-pool".into(),
                    profile_id: profile_id.parse()?,
                    target_id: target.parse()?,
                    workload_id: WorkloadId::try_new(Uuid::new_v4())?,
                    generation: Generation::new(1)?,
                    kind: WorkloadKind::CiRunner,
                    agent_run_id: None,
                    expected_profile_revision: Some(1),
                },
            )
            .await?;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let considered = || async {
        service.admit_queued(&Principal::Reconciler, 1).await?;
        let repositories: Vec<i64> = sqlx::query_scalar(
            "SELECT t.repository_id FROM workload_intents i JOIN fleet_ci_targets t ON t.id=i.target_id
              WHERE i.reason_code IS NOT NULL ORDER BY i.created_at",
        )
        .fetch_all(&f.database)
        .await?;
        sqlx::query("UPDATE workload_intents SET reason_code=NULL")
            .execute(&f.database)
            .await?;
        Ok::<_, anyhow::Error>(repositories)
    };
    assert_eq!(considered().await?, [11]);

    // A job that has waited past the pool's priority window goes ahead of the
    // prioritised repository's newer one, so priority can't starve the pool.
    sqlx::query("UPDATE workflow_jobs SET created_at=? WHERE id=50")
        .bind(now - 11 * 60_000)
        .execute(&f.database)
        .await?;
    assert_eq!(next_repository().await?, 10);
    assert_eq!(considered().await?, [10]);
    Ok(())
}

#[tokio::test]
async fn overview_charts_each_machines_workload() -> Result<()> {
    let f = Fixture::new().await?;
    seed_pool(&f).await?;
    let machine = active_machine(&f, &f.origin, Arc::new(FakeDocker::default())).await?;
    prepared_placement(&f, &machine, Uuid::new_v4()).await?;
    // Two hours ago, before the agent's own samples: full CPU load and half
    // the machine's memory.
    let (domain, epoch, cpu, memory): (String, i64, i64, i64) = sqlx::query_as(
        "SELECT d.id,h.epoch,d.cpu_millis,d.memory_mib FROM host_resource_domains d
           JOIN fleet_hosts h ON h.id=d.host_id WHERE d.host_id=? AND d.parent_domain_id IS NULL",
    )
    .bind(machine.host.to_string())
    .fetch_one(&f.database)
    .await?;
    let earlier = gridops_core::now_millis() - 2 * 3_600_000;
    sqlx::query(
        "INSERT INTO host_samples (host_id,domain_id,host_epoch,observed_at,received_at,cpu_used_millis,memory_used_mib,coverage)
         VALUES (?,?,?,?,?,?,?,'complete')",
    )
    .bind(machine.host.to_string())
    .bind(&domain)
    .bind(epoch)
    .bind(earlier)
    .bind(earlier)
    .bind(cpu)
    .bind(memory / 2)
    .execute(&f.database)
    .await?;

    let workload: Value = f
        .client
        .get(f.url("/workload?window=24h"))
        .header(header::COOKIE, &f.cookie)
        .send()
        .await?
        .json()
        .await?;
    let machines = workload["machines"].as_array().context("machines")?;
    let slot = machines
        .iter()
        .position(|item| item["id"] == machine.host.to_string())
        .context("the machine is charted")?;
    let points = workload["points"].as_array().context("points")?;
    assert_eq!(points.len(), 24 * 6 + 1, "ten-minute buckets over a day");
    let latest = &points.last().context("latest")?["machines"][slot];
    assert_eq!(latest["runners"], 1.0, "the runner on the machine now");
    let sampled = points
        .iter()
        .map(|point| &point["machines"][slot])
        .find(|load| load["cpuPercent"] == 100.0)
        .context("the bucket with the sample")?;
    assert!((sampled["memoryPercent"].as_f64().context("memory")? - 50.0).abs() <= 0.1);
    // A day before the placement existed, the machine was idle.
    assert_eq!(points[0]["machines"][slot]["runners"], 0.0);

    let rejected = f
        .client
        .get(f.url("/workload?window=1y"))
        .header(header::COOKIE, &f.cookie)
        .send()
        .await?;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

/// The overview names the machine that runs the same jobs faster, and counts
/// each machine's jobs, failures and typical job time.
#[tokio::test]
async fn overview_compares_machines_on_the_same_jobs() -> Result<()> {
    let f = Fixture::new().await?;
    seed_pool(&f).await?;
    // One runner on each machine: the second machine joins once the first
    // has its runner, and the first stops taking more.
    let slow = active_machine(&f, &f.origin, Arc::new(FakeDocker::default())).await?;
    let slow_runner = Uuid::new_v4();
    admitted_placement(&f, &slow, slow_runner).await?;
    let fast = active_machine(&f, &f.origin, Arc::new(FakeDocker::default())).await?;
    sqlx::query("UPDATE host_backends SET enabled=0 WHERE host_id=?")
        .bind(slow.host.to_string())
        .execute(&f.database)
        .await?;
    let fast_runner = Uuid::new_v4();
    admitted_placement(&f, &fast, fast_runner).await?;
    for (runner, name) in [(slow_runner, "runner-slow"), (fast_runner, "runner-fast")] {
        sqlx::query("UPDATE runners SET name=? WHERE id=?")
            .bind(name)
            .bind(runner.to_string())
            .execute(&f.database)
            .await?;
    }
    // The same "test" job takes 120 s on one machine and 60 s on the other;
    // the slow machine also failed a build, and GitHub ran a job itself.
    let now = gridops_core::now_millis();
    sqlx::query(
        "INSERT INTO workflow_runs (id,repository_id,workflow_name,run_number,event,status,head_sha,html_url,github_created_at,github_updated_at,created_at,updated_at)
         VALUES (20,10,'CI',1,'push','completed','abc','https://github.com/octo-org/gridops/actions/runs/20',?,?,?,?)",
    )
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&f.database)
    .await?;
    let jobs = [
        ("test", "runner-slow", "success", 120),
        ("test", "runner-slow", "success", 120),
        ("test", "runner-slow", "success", 120),
        ("build", "runner-slow", "failure", 30),
        ("test", "runner-fast", "success", 60),
        ("test", "runner-fast", "success", 60),
        ("test", "runner-fast", "success", 60),
        ("test", "GitHub Actions 1", "success", 10),
    ];
    for (index, (name, runner, conclusion, seconds)) in jobs.into_iter().enumerate() {
        let started = now - 3_600_000;
        sqlx::query(
            "INSERT INTO workflow_jobs (id,run_id,name,status,conclusion,labels,html_url,runner_name,started_at,completed_at,created_at,updated_at)
             VALUES (?,20,?,'completed',?,'[\"self-hosted\"]','https://github.com/octo-org/gridops/actions/runs/20',?,?,?,?,?)",
        )
        .bind(100 + i64::try_from(index)?)
        .bind(name)
        .bind(conclusion)
        .bind(runner)
        .bind(started)
        .bind(started + seconds * 1_000)
        .bind(started)
        .bind(started)
        .execute(&f.database)
        .await?;
    }

    let overview: Value = f
        .client
        .get(format!("{}/api/v1/overview", f.origin))
        .header(header::COOKIE, &f.cookie)
        .send()
        .await?
        .json()
        .await?;
    let machines = overview["machines"]["items"]
        .as_array()
        .context("machines")?;
    let shown = |host: &gridops_core::fleet::ids::HostId| {
        machines
            .iter()
            .find(|machine| machine["id"] == host.to_string())
            .map(|machine| {
                (
                    machine["jobs"].as_i64(),
                    machine["failed"].as_i64(),
                    machine["medianSeconds"].as_i64(),
                    machine["relativePercent"].as_i64(),
                    machine["fastest"].as_bool(),
                )
            })
    };
    assert_eq!(machines.len(), 2, "GitHub's own runners aren't machines");
    assert_eq!(
        shown(&slow.host),
        Some((Some(4), Some(1), Some(120), Some(200), Some(false)))
    );
    assert_eq!(
        shown(&fast.host),
        Some((Some(3), Some(0), Some(60), Some(100), Some(true)))
    );
    Ok(())
}

/// A runner goes to the machine fewer other platforms run on, even with less
/// room, so a machine that other work may need stays free for it. Between
/// machines needed equally, the roomier one wins.
#[tokio::test]
async fn placement_prefers_the_least_needed_machine_then_the_roomiest() -> Result<()> {
    let f = Fixture::new().await?;
    seed_pool(&f).await?;
    let platforms = json!([
        {"runtimeKind": "docker", "executionOs": "linux", "architecture": "arm64", "image": "runner:fixture"},
        {"runtimeKind": "native_process", "executionOs": "linux", "architecture": "arm64", "image": "native:actions-runner"},
    ]);
    let mut shared = active_machine_on(
        &f,
        &f.origin,
        Arc::new(FakeDocker::default()),
        platforms.clone(),
    )
    .await?;
    let mut dedicated =
        active_machine_on(&f, &f.origin, Arc::new(FakeDocker::default()), platforms).await?;
    let docker: String = sqlx::query_scalar(
        "SELECT id FROM pool_execution_profiles
          WHERE pool_id='fleet-pool' AND runtime_kind='docker' AND enabled=1 AND archived_at IS NULL",
    )
    .fetch_one(&f.database)
    .await?;
    shared.profile_id.clone_from(&docker);
    dedicated.profile_id = docker;
    // The shared machine can also run the pool's native platform.
    sqlx::query(
        "INSERT INTO host_backends (id,host_id,domain_id,runtime_kind,execution_os,architecture,readiness,enabled,created_at,updated_at)
         SELECT ?,host_id,domain_id,'native_process',execution_os,architecture,'ready',1,1,1
           FROM host_backends WHERE host_id=?",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(shared.host.to_string())
    .execute(&f.database)
    .await?;
    let placed_on = |placement: PlacementId| {
        let database = f.database.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT host_id FROM workload_placements WHERE id=?")
                .bind(placement.to_string())
                .fetch_one(&database)
                .await
        }
    };
    // The dedicated machine takes two runners. The default budgets follow the
    // test host's CPU count, which on a 2-CPU runner fits only one, so an
    // administrator sets the same budget for two on both machines.
    sqlx::query(
        "UPDATE host_resource_domains SET cpu_millis=4000,revision=revision+1
          WHERE host_id IN (?,?) AND parent_domain_id IS NULL",
    )
    .bind(shared.host.to_string())
    .bind(dedicated.host.to_string())
    .execute(&f.database)
    .await?;
    for _ in 0..2 {
        let placement = admitted_placement(&f, &dedicated, Uuid::new_v4()).await?;
        assert_eq!(placed_on(placement).await?, dedicated.host.to_string());
    }
    // Nothing else needs the shared machine now, and it has more room.
    sqlx::query(
        "UPDATE host_backends SET enabled=0 WHERE host_id=? AND runtime_kind='native_process'",
    )
    .bind(shared.host.to_string())
    .execute(&f.database)
    .await?;
    let placement = admitted_placement(&f, &shared, Uuid::new_v4()).await?;
    assert_eq!(placed_on(placement).await?, shared.host.to_string());
    Ok(())
}

/// The overview breaks a fleet runner's start into its phases, and shows how
/// long reconciler passes take, since a prepared runner waits for one.
#[tokio::test]
async fn overview_times_each_phase_of_a_runner_start() -> Result<()> {
    let f = Fixture::new().await?;
    seed_pool(&f).await?;
    let machine = active_machine(&f, &f.origin, Arc::new(FakeDocker::default())).await?;
    let workload = Uuid::new_v4();
    let placement_id = prepared_placement(&f, &machine, workload).await?;
    issue_start(&f, placement_id, workload, 55_000).await?;
    eventually("running runner", || async {
        Ok((placement_state(&f, placement_id).await? == "running").then_some(()))
    })
    .await?;
    // The runner was requested five minutes before a machine had room, and
    // took a job fifteen seconds after it was told to start. Commands keep
    // the times they were really sent and answered.
    let (placed, prepare_sent, prepared, start_sent): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT pl.created_at,prepare.issued_at,
                (SELECT MIN(r.received_at) FROM host_command_results r WHERE r.command_id=prepare.id),
                start.issued_at
           FROM workload_placements pl
           JOIN host_commands prepare ON prepare.placement_id=pl.id AND prepare.kind='prepare_environment'
           JOIN host_commands start ON start.placement_id=pl.id AND start.kind='start_runner'
          WHERE pl.id=?",
    )
    .bind(placement_id.to_string())
    .fetch_one(&f.database)
    .await?;
    let requested = placed - 300_000;
    sqlx::query("UPDATE workload_intents SET created_at=? WHERE workload_id=?")
        .bind(requested)
        .bind(workload.to_string())
        .execute(&f.database)
        .await?;
    sqlx::query(
        "INSERT INTO runner_events (id,runner_id,pool_id,event,message,created_at)
         VALUES (?,?,'fleet-pool','Workflow job in_progress','build',?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(workload.to_string())
    .bind(start_sent + 15_000)
    .execute(&f.database)
    .await?;
    let now = gridops_core::now_millis();
    sqlx::query("INSERT INTO reconciler_passes (started_at,duration_ms) VALUES (?,1500),(?,90000)")
        .bind(now - 60_000)
        .bind(now - 30_000)
        .execute(&f.database)
        .await?;

    let overview: Value = f
        .client
        .get(format!("{}/api/v1/overview", f.origin))
        .header(header::COOKIE, &f.cookie)
        .send()
        .await?
        .json()
        .await?;
    let starts = &overview["slo"]["runnerStarts"];
    let phases = starts["phases"]
        .as_array()
        .context("phases")?
        .iter()
        .map(|phase| {
            (
                phase["id"].as_str().unwrap_or_default(),
                phase["p50Seconds"].as_i64(),
            )
        })
        .collect::<Vec<_>>();
    let seconds = |millis: i64| Some((millis + 999) / 1_000);
    assert_eq!(
        phases,
        [
            ("room", Some(300)),
            ("preparing", seconds(prepared - prepare_sent)),
            ("waitingToStart", seconds(start_sent - prepared)),
            ("starting", Some(15)),
        ]
    );
    assert_eq!(
        starts["total"]["p50Seconds"].as_i64(),
        seconds(start_sent + 15_000 - requested)
    );
    assert_eq!(starts["total"]["sampleSize"], 1);
    let pass = &overview["slo"]["reconcilerPass"];
    assert_eq!(pass["sampleSize"], 2);
    assert_eq!(pass["p50Seconds"], 2, "1.5s, rounded up");
    assert_eq!(pass["p95Seconds"], 90);
    assert_eq!(pass["intervalSeconds"], 30);
    Ok(())
}
