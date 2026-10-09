-- Exercises the pre-fleet relationships that additive migrations must preserve.
-- IDs deliberately include legacy non-UUID values; they are not new fleet IDs.
INSERT INTO users (id,github_id,login,access_token,last_login_at,created_at,updated_at,role)
VALUES ('legacy-admin',123,'fixture-user','sealed-fixture',1,1,1,'admin');
INSERT INTO installations (id,account_id,account_login,account_type,target_type,repository_selection,created_at,updated_at)
VALUES (1,123,'fixture-user','User','User','selected',1,1),
       (2,456,'fixture-org','Organization','Organization','selected',1,1);
INSERT INTO user_installations (user_id,installation_id,permission,created_at)
VALUES ('legacy-admin',1,'admin',1),('legacy-admin',2,'admin',1);
INSERT INTO repositories (id,installation_id,owner,name,full_name,private,default_branch,html_url,last_synced_at,created_at,updated_at)
VALUES (10,1,'fixture-user','fleet-tests','fixture-user/fleet-tests',1,'main','https://example.invalid/repo',1,1,1),
       (11,2,'fixture-org','native-tests','fixture-org/native-tests',1,'main','https://example.invalid/native',1,1,1);
INSERT INTO runner_pools (id,installation_id,name,scope,labels,image,provider,providers,macos_runtime,created_at,updated_at)
VALUES ('legacy-pool',1,'Mixed legacy','repository','["self-hosted"]','runner:fixture','docker','["docker","tart"]','native',1,1);
INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at) VALUES ('legacy-pool',10,1),('legacy-pool',11,1);
INSERT INTO bitbucket_connections (id,name,workspace,workspace_uuid,access_token_key,created_by,created_at,updated_at)
VALUES ('legacy-bb','Fixture workspace','fixture-workspace','{11111111-1111-4111-8111-111111111111}','fixture-bb-key','legacy-admin',1,1);
INSERT INTO runner_pool_bitbucket_connections (pool_id,connection_id,created_at) VALUES ('legacy-pool','legacy-bb',1);
INSERT INTO runners (id,pool_id,target_repository_id,github_runner_id,name,container_id,status,provider,created_at,updated_at)
VALUES ('legacy-runner','legacy-pool',10,100,'existing-runner','existing-container','online','docker',1,1);
INSERT INTO runners (id,pool_id,target_repository_id,github_runner_id,name,status,provider,runtime,os,architecture,created_at,updated_at)
VALUES ('legacy-native','legacy-pool',11,101,'existing-native','online','tart','native','macos','arm64',1,1);
INSERT INTO runners (id,pool_id,name,status,provider,ci_platform,bitbucket_connection_id,bitbucket_runner_uuid,created_at,updated_at)
VALUES ('legacy-bb-runner','legacy-pool','existing-bb','online','tart','bitbucket','legacy-bb','{22222222-2222-4222-8222-222222222222}',1,1);
INSERT INTO workflow_runs (id,repository_id,workflow_id,workflow_name,run_number,run_attempt,event,status,head_branch,head_sha,actor_login,html_url,github_created_at,github_updated_at,created_at,updated_at)
VALUES (20,10,5,'Fixture workflow',1,1,'push','completed','main','fixture-sha','fixture-user','https://example.invalid/run',1,1,1,1);
INSERT INTO workflow_jobs (id,run_id,name,status,conclusion,runner_id,runner_name,html_url,created_at,updated_at)
VALUES (30,20,'Fixture job','completed','failure',100,'existing-runner','https://example.invalid/job',1,1);
INSERT INTO runner_events (id,runner_id,pool_id,event,message,created_at)
VALUES ('legacy-event','legacy-runner','legacy-pool','online','Fixture history',1);
INSERT INTO log_streams (id,job_id,runner_id,source,path,installation_id,size_bytes,complete,created_at,updated_at)
VALUES ('legacy-log',30,'legacy-runner','github','fixture/log',1,32,1,1,1);
INSERT INTO agent_runs (id,job_id,run_id,repository_id,trigger,status,requested_by,created_at,updated_at)
VALUES ('legacy-fix',30,20,10,'manual','succeeded','legacy-admin',1,1);
INSERT INTO agent_run_events (agent_run_id,kind,title,created_at)
VALUES ('legacy-fix','result','Fixture diagnosis',1);
INSERT INTO runtime_secrets (key,value,updated_by,updated_at) VALUES ('fixture-bb-key','sealed-fixture','legacy-admin',1);
