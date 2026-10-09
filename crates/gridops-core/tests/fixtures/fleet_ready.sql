-- Synthetic ownership graph for database constraint tests, never enrollment.
INSERT INTO fleet_hosts (id,name,host_os,architecture,enrollment_state,lifecycle_state,integrity_state,created_at,updated_at)
VALUES ('10000000-0000-4000-8000-000000000001','Mac fixture','macos','arm64','approved','active','verified',1,1),
       ('20000000-0000-4000-8000-000000000001','Windows fixture','windows','x64','approved','active','verified',1,1),
       ('90000000-0000-4000-8000-000000000001','Linux fixture','linux','x64','approved','active','verified',1,1);
INSERT INTO host_resource_domains (id,host_id,parent_domain_id,name,cpu_millis,memory_mib,disk_bytes)
VALUES ('11000000-0000-4000-8000-000000000001','10000000-0000-4000-8000-000000000001',NULL,'Physical',4000,8192,107374182400),
       ('12000000-0000-4000-8000-000000000001','10000000-0000-4000-8000-000000000001','11000000-0000-4000-8000-000000000001','Docker VM',2000,4096,53687091200),
       ('21000000-0000-4000-8000-000000000001','20000000-0000-4000-8000-000000000001',NULL,'Physical',4000,8192,107374182400),
       ('22000000-0000-4000-8000-000000000001','20000000-0000-4000-8000-000000000001','21000000-0000-4000-8000-000000000001','Linux VM',2000,4096,53687091200),
       ('91000000-0000-4000-8000-000000000001','90000000-0000-4000-8000-000000000001',NULL,'Physical',4000,8192,107374182400);
INSERT INTO host_backends (id,host_id,domain_id,runtime_kind,execution_os,architecture,readiness,enabled,created_at,updated_at)
VALUES ('13000000-0000-4000-8000-000000000001','10000000-0000-4000-8000-000000000001','12000000-0000-4000-8000-000000000001','docker','linux','arm64','ready',1,1,1),
       ('14000000-0000-4000-8000-000000000001','10000000-0000-4000-8000-000000000001','11000000-0000-4000-8000-000000000001','native_process','macos','arm64','ready',1,1,1),
       ('23000000-0000-4000-8000-000000000001','20000000-0000-4000-8000-000000000001','22000000-0000-4000-8000-000000000001','docker','linux','x64','ready',1,1,1),
       ('24000000-0000-4000-8000-000000000001','20000000-0000-4000-8000-000000000001','21000000-0000-4000-8000-000000000001','native_process','windows','x64','ready',1,1,1),
       ('93000000-0000-4000-8000-000000000001','90000000-0000-4000-8000-000000000001','91000000-0000-4000-8000-000000000001','docker','linux','x64','ready',1,1,1);
INSERT INTO fleet_ci_targets (id,kind,canonical_key,installation_id,repository_id)
VALUES ('30000000-0000-4000-8000-000000000001','github_repository','github:1:10',1,10);
INSERT INTO fleet_ci_targets (id,kind,canonical_key,bitbucket_connection_id,workspace_uuid)
VALUES ('30000000-0000-4000-8000-000000000002','bitbucket_workspace','bitbucket:legacy-bb:11111111-1111-4111-8111-111111111111','legacy-bb','11111111-1111-4111-8111-111111111111');
INSERT INTO pool_execution_profiles (id,pool_id,name,enabled,runtime_kind,execution_os,architecture,mode,image,cpu_millis,memory_mib,disk_bytes,created_at,updated_at)
VALUES ('40000000-0000-4000-8000-000000000001','legacy-pool','Docker ARM64',1,'docker','linux','arm64','ephemeral','runner:fixture',1000,1024,1073741824,1,1);
INSERT INTO profile_ci_targets (profile_id,target_id)
VALUES ('40000000-0000-4000-8000-000000000001','30000000-0000-4000-8000-000000000001');
INSERT INTO fleet_operations (id,kind,requested_by_user_id,principal_kind,principal_scope,idempotency_key,request_hash,created_at,updated_at)
VALUES ('50000000-0000-4000-8000-000000000001','submit_workload','legacy-admin','user','user:legacy-admin','fixture-idempotency','fixture-hash',1,1);
INSERT INTO workload_intents (id,workload_id,generation,kind,pool_id,profile_id,target_id,expected_profile_revision,created_at,updated_at)
VALUES ('50000000-0000-4000-8000-000000000001','51000000-0000-4000-8000-000000000001',1,'ci_runner','legacy-pool','40000000-0000-4000-8000-000000000001','30000000-0000-4000-8000-000000000001',1,1,1);
INSERT INTO workload_placements (id,intent_id,workload_id,generation,host_id,backend_id,host_epoch,profile_revision,target_id,config_snapshot_json,cpu_millis,memory_mib,disk_bytes,state,created_at,updated_at)
VALUES ('60000000-0000-4000-8000-000000000001','50000000-0000-4000-8000-000000000001','51000000-0000-4000-8000-000000000001',1,'10000000-0000-4000-8000-000000000001','13000000-0000-4000-8000-000000000001',1,1,'30000000-0000-4000-8000-000000000001','{}',1000,1024,1073741824,'reserved',1,1);
INSERT INTO capacity_allocations (id,placement_id,host_id,backend_id,host_epoch,domain_id,policy_revision,cpu_millis,memory_mib,disk_bytes,created_at,updated_at)
VALUES ('61000000-0000-4000-8000-000000000001','60000000-0000-4000-8000-000000000001','10000000-0000-4000-8000-000000000001','13000000-0000-4000-8000-000000000001',1,'11000000-0000-4000-8000-000000000001',1,1000,1024,1073741824,1,1),
       ('61000000-0000-4000-8000-000000000002','60000000-0000-4000-8000-000000000001','10000000-0000-4000-8000-000000000001','13000000-0000-4000-8000-000000000001',1,'12000000-0000-4000-8000-000000000001',1,1000,1024,1073741824,1,1);
INSERT INTO host_commands (id,operation_id,scope,placement_id,host_id,backend_id,host_epoch,generation,kind,protocol_version,request_hash,payload_json,expected_revision,issued_at,deadline_at,idempotency_key)
VALUES ('70000000-0000-4000-8000-000000000001','50000000-0000-4000-8000-000000000001','placement','60000000-0000-4000-8000-000000000001','10000000-0000-4000-8000-000000000001','13000000-0000-4000-8000-000000000001',1,1,'prepare_environment',1,'fixture-command-hash','{}',1,1,1200001,'fixture-command');
