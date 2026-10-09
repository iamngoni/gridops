# Spec Delta

## Purpose

This capability connects approved Linux, macOS, and Windows hosts to one control
plane with scoped credentials, explicit protocol compatibility, and recoverable
outbound communication.

## ADDED Requirements

### Requirement: Enrollment is short-lived, scoped, and approved
The system SHALL issue a single-use, expiring enrollment instruction bound to a host scope and intended targets, accept it only through protected installer input, and require administrator approval of observed capabilities before scheduling.

#### Scenario: Installer enrolls a new host
- **WHEN** a host presents an unexpired instruction over the supported enrollment endpoint
- **THEN** the system creates a pending host identity, returns an opaque host credential, and does not schedule work until approval

#### Scenario: Code is replayed
- **WHEN** an already-consumed or expired instruction is presented
- **THEN** enrollment is rejected with a stable expired-or-used error and no credential is issued

#### Scenario: Scoped autoapproval is used
- **WHEN** an administrator enables autoapproval for no more than one hour with a bounded host count and named targets, pools, and capabilities, and a matching host enrolls
- **THEN** only the declared scope is approved, native or interactive trust is not expanded, and the use is audited

### Requirement: Agent authentication is host-scoped
The system MUST store only a verifier for a high-entropy host credential, protect its local copy with the OS account/keychain/ACL boundary, and reject requests whose credential generation is revoked, mismatched, or presented for another host.

#### Scenario: Revoked credential reconnects
- **WHEN** a host sends a heartbeat after an administrator revokes its credential generation
- **THEN** the request is rejected, queued starts for that generation are invalidated, and no browser-admin route is exposed

#### Scenario: Rotation overlaps safely
- **WHEN** the administrator rotates credentials and the agent acknowledges the new generation
- **THEN** the old generation is accepted only during the recorded overlap and is rejected after acknowledgement or expiry

#### Scenario: A currently valid credential is stolen
- **WHEN** a copied currently-valid credential is reported or a conflicting session is detected
- **THEN** administrators can quarantine the conflicting session and revoke/rotate/recover the host; reported copying or conflicting use is compromise evidence; ordinary possession of a valid credential is not hardware attestation and does not automatically identify an attacker

### Requirement: Connectivity uses bounded outbound protocol
The system SHALL support outbound HTTPS with certificate validation, explicit proxy/CA settings, a maximum 25-second command long poll, 15-second heartbeats, acknowledged event batches, and resumable bounded log/artifact chunks without requiring inbound host ports.

#### Scenario: NATed laptop connects
- **WHEN** a laptop can reach the control-plane HTTPS URL through NAT or an explicit proxy
- **THEN** commands, heartbeats, events, and logs flow outbound and no inbound port or automatic firewall/VPN change is required

#### Scenario: TLS validation fails
- **WHEN** the configured certificate or CA cannot validate
- **THEN** the agent remains disconnected with a diagnostic and never falls back silently to insecure remote HTTP

### Requirement: Protocol compatibility is negotiated before work
The system MUST exchange supported protocol ranges and agent capabilities during enrollment and reconnect, reject incompatible operations with a stable reason, and keep the host in maintenance until a compatible upgrade is installed.

#### Scenario: Older agent reconnects
- **WHEN** an agent supports no version accepted by the control plane
- **THEN** the handshake is rejected, pending commands remain durable, and the host cannot resume scheduling

#### Scenario: Compatible minor version reconnects
- **WHEN** the intersection of protocol ranges includes the required version
- **THEN** the session records that version and the agent may exchange only operations valid for it

### Requirement: Telemetry and event replay are bounded and acknowledged
The system SHALL persist event cursors and acknowledgments, enforce payload/batch/spool quotas, and resume from the last acknowledged cursor without duplicating durable events.

#### Scenario: Connection drops mid-batch
- **WHEN** an agent reconnects after sending an unacknowledged event batch
- **THEN** it resends from the last acknowledged cursor and the server deduplicates event IDs

#### Scenario: Spool quota is reached
- **WHEN** offline logs exceed the configured bounded spool quota
- **THEN** the agent applies the documented retention policy, reports loss explicitly, and never blocks command fencing or hides the gap
