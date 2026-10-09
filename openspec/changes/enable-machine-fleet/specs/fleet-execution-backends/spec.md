# Spec Delta

## Purpose

This capability provides explicit, honest execution adapters for supported
platform and CI-target combinations, including native and authorized interactive
sessions without claiming isolation the host cannot enforce.

## ADDED Requirements

### Requirement: Backend selection follows a versioned compatibility matrix
The system SHALL define and implement every committed backend/target/architecture row, expose pending evidence separately from unsupported, and return an explicit unsupported reason only for a genuinely unsupported upstream combination.

#### Scenario: Linux Docker serves both targets
- **WHEN** a Linux x64 or ARM64 Docker backend passes its prerequisite probes
- **THEN** GitHub Docker runners and the supported Bitbucket Linux Docker adapter can be selected independently

The committed delivery matrix includes GitHub Linux Docker/native x64 and ARM64
where upstream supports, macOS native x64/ARM64 and Apple Silicon Tart VM,
Windows native x64/ARM64 headless/interactive and the currently available GitHub
Windows ARM64 artifact, plus Bitbucket Linux Docker x64/ARM64, upstream-supported
Linux Shell architectures, macOS native/Tart Shell, and Windows x64 PowerShell.
Bitbucket Windows ARM64 is excluded only while genuinely upstream-unsupported;
missing execution hardware leaves every other row pending and blocks acceptance.

#### Scenario: Windows ARM64 Bitbucket is requested
- **WHEN** a pool requests Bitbucket on Windows ARM64 before upstream support is proven
- **THEN** eligibility is rejected as unsupported and the UI does not infer support from a GitHub artifact

#### Scenario: Linux native row executes
- **WHEN** a Linux x64 or ARM64 native backend passes its upstream and trust probes
- **THEN** a GitHub native runner and an upstream-supported Bitbucket Linux Shell runner can execute with the declared shared-host boundary

#### Scenario: macOS rows execute
- **WHEN** a macOS native x64/ARM64 backend or Apple Silicon Tart VM passes session/TCC and runtime probes
- **THEN** GitHub native/Tart ephemeral runners and Bitbucket native/Tart persistent Shell runners use the corresponding backend, with Tart GitHub lifecycle remaining ephemeral

#### Scenario: Windows rows execute
- **WHEN** Windows x64 headless or authorized interactive execution passes service/session probes
- **THEN** GitHub and Bitbucket PowerShell jobs execute under the declared account/session boundary, while a currently available GitHub Windows ARM64 artifact remains a required execution-evidence row

### Requirement: Native execution declares its trust boundary
The system MUST require an administrator-approved trusted target for native Linux/macOS/Windows work, use per-workload directories and process trees, and state that a private directory is not a sandbox or host-mutation guarantee.

#### Scenario: Untrusted pool requests native shell
- **WHEN** a pool without a native trust grant requests a native backend
- **THEN** placement is denied with a missing-trust reason and no process is started

#### Scenario: Native workload ends
- **WHEN** a native process exits after a job
- **THEN** its workload directory and process-tree result are recorded per placement, while the system does not claim that host secrets or arbitrary mutations were erased

### Requirement: Linux and macOS adapters expose enforcement accurately
The system SHALL use cgroups where available for Linux resource enforcement, report macOS native CPU/memory as reservation unless hard enforcement is proven, and preserve the existing Tart VM behavior as a child backend with its own budget.

#### Scenario: macOS TCC grant is revoked
- **WHEN** a required logged-in session or TCC permission disappears
- **THEN** the affected backend becomes unavailable for new interactive placement while unrelated Tart or headless backends remain eligible

#### Scenario: Tart VM is selected
- **WHEN** a supported Apple Silicon host selects a Tart backend
- **THEN** the placement is labeled macOS VM execution, consumes both VM and physical host budgets, uses ephemeral lifecycle for GitHub and persistent lifecycle for Bitbucket Shell

### Requirement: Windows service and interactive execution are separate
The system MUST run headless Windows work under a configured non-admin service account with ACL-scoped storage and Job Object/process-tree supervision; interactive work requires an installed authorized user-session helper and an available session.

#### Scenario: GUI work arrives in Session 0
- **WHEN** a profile requests desktop UI but only the Windows service session is available
- **THEN** the backend is ineligible with a session-required reason and no automatic login or privilege escalation occurs

#### Scenario: Interactive user logs out
- **WHEN** the authorized interactive session ends while no job is running
- **THEN** new interactive starts pause until a fresh session health check succeeds

### Requirement: CI registration mode matches upstream lifecycle
The system SHALL keep GitHub JIT/registration and Bitbucket workspace/repository registration in target adapters, SHALL treat Bitbucket modes as persistent unless an upstream lifecycle contract is proven, and SHALL send only minimum bootstrap material to the selected host.

#### Scenario: Bitbucket registration secret is requested
- **WHEN** a persistent Bitbucket placement starts
- **THEN** the adapter records the target scope and lifecycle mode and does not emulate ephemeral behavior by adding unsupported flags

#### Scenario: Bootstrap is expired
- **WHEN** a host receives an expired or consumed registration bootstrap
- **THEN** the start fails as retryable or terminal per adapter evidence and a fresh scoped attempt is required
