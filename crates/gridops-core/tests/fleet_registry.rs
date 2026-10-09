//! Focused registry boundary tests.
//!
//! These tests exercise bounded scope/secret parsing and prove that the
//! additive registry migration is present on a real file-backed `SQLite` DB.
//! End-to-end service transition tests are covered by the crate-private
//! registry test module; this file keeps migration and serialization checks
//! on a separately compiled real `SQLite` database.

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use gridops_core::{connect_database_path, fleet::registry::EnrollmentScope};
use sqlx::Row as _;
use uuid::Uuid;

#[test]
fn enrollment_scope_rejects_duplicate_targets_and_unknown_fields() -> Result<()> {
    let target = Uuid::parse_str("30000000-0000-4000-8000-000000000001")?;
    let duplicate = serde_json::json!({
        "target_ids": [target, target]
    });
    assert!(serde_json::from_value::<EnrollmentScope>(duplicate).is_err());
    let unknown = serde_json::json!({
        "target_ids": [target],
        "allow_native": true
    });
    assert!(serde_json::from_value::<EnrollmentScope>(unknown).is_err());
    Ok(())
}

#[tokio::test]
async fn registry_migration_is_additive_and_verifier_only() -> Result<()> {
    let directory = std::env::temp_dir().join(format!("gridops-registry-{}", Uuid::new_v4()));
    let pool = connect_database_path(&directory.join("registry.sqlite")).await?;
    let columns = sqlx::query("PRAGMA table_info(host_credential_rotations)")
        .fetch_all(&pool)
        .await?;
    let names = columns
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
    assert!(names.iter().any(|name| name == "proposed_verifier"));
    assert!(names.iter().any(|name| name == "next_generation"));
    assert!(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name='host_credentials_verifier_format_insert'",
    )
    .fetch_one(&pool)
    .await?
        == 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name='host_enrollments_issued_identity_immutable'",
        )
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name='host_credential_rotation_request_identity_immutable'",
        )
        .fetch_one(&pool)
        .await?,
        1
    );
    pool.close().await;
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

#[tokio::test]
async fn rotation_schema_binds_host_generation_and_phase() -> Result<()> {
    let directory =
        std::env::temp_dir().join(format!("gridops-registry-schema-{}", Uuid::new_v4()));
    let pool = connect_database_path(&directory.join("registry.sqlite")).await?;
    sqlx::query(
        "INSERT INTO users
         (id,github_id,login,role,access_token,last_login_at,created_at,updated_at)
         VALUES ('schema-admin',900,'schema-admin','admin','',1,1,1)",
    )
    .execute(&pool)
    .await?;
    for (id, name) in [
        ("a0000000-0000-4000-8000-000000000001", "host-a"),
        ("b0000000-0000-4000-8000-000000000001", "host-b"),
    ] {
        sqlx::query(
            "INSERT INTO fleet_hosts (id,name,host_os,architecture,created_at,updated_at)
             VALUES (? ,?,'linux','x64',1,1)",
        )
        .bind(id)
        .bind(name)
        .execute(&pool)
        .await?;
    }
    let verifier_a = URL_SAFE_NO_PAD.encode([1_u8; 32]);
    let verifier_b = URL_SAFE_NO_PAD.encode([2_u8; 32]);
    sqlx::query(
        "INSERT INTO host_credentials
         (id,host_id,host_epoch,generation,verifier,verifier_format,created_at)
         VALUES ('ca000000-0000-4000-8000-000000000001',? ,1,1,?,'sha256',1),
                ('cb000000-0000-4000-8000-000000000001',? ,1,1,?,'sha256',1)",
    )
    .bind("a0000000-0000-4000-8000-000000000001")
    .bind(verifier_a)
    .bind("b0000000-0000-4000-8000-000000000001")
    .bind(verifier_b)
    .execute(&pool)
    .await?;
    let cross_host = sqlx::query(
        "INSERT INTO host_credential_rotations
         (id,host_id,old_credential_id,old_generation,old_host_epoch,next_generation,idempotency_key,
          requested_by,request_method,request_resource,request_hash,phase,requested_at,expires_at)
         VALUES ('ra000000-0000-4000-8000-000000000001',?,'cb000000-0000-4000-8000-000000000001',1,1,2,
                 'cross','schema-admin','fleet.credential.request',?,
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','requested',1,2)",
    )
    .bind("a0000000-0000-4000-8000-000000000001")
    .bind("a0000000-0000-4000-8000-000000000001")
    .execute(&pool)
    .await;
    assert!(cross_host.is_err());
    let wrong_generation = sqlx::query(
        "INSERT INTO host_credential_rotations
         (id,host_id,old_credential_id,old_generation,old_host_epoch,next_generation,idempotency_key,
          requested_by,request_method,request_resource,request_hash,phase,requested_at,expires_at)
         VALUES ('rg000000-0000-4000-8000-000000000001',?,'ca000000-0000-4000-8000-000000000001',2,1,3,
                 'wrong-generation','schema-admin','fleet.credential.request',?,
                 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','requested',1,2)",
    )
    .bind("a0000000-0000-4000-8000-000000000001")
    .bind("a0000000-0000-4000-8000-000000000001")
    .execute(&pool)
    .await;
    assert!(wrong_generation.is_err());
    sqlx::query(
        "INSERT INTO host_credential_rotations
         (id,host_id,old_credential_id,old_generation,old_host_epoch,next_generation,idempotency_key,
          requested_by,request_method,request_resource,request_hash,phase,requested_at,expires_at)
         VALUES ('ra000000-0000-4000-8000-000000000001',?,'ca000000-0000-4000-8000-000000000001',1,1,2,
                 'valid','schema-admin','fleet.credential.request',?,
                 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc','requested',1,2)",
    )
    .bind("a0000000-0000-4000-8000-000000000001")
    .bind("a0000000-0000-4000-8000-000000000001")
    .execute(&pool)
    .await?;
    let malformed_phase = sqlx::query(
        "UPDATE host_credential_rotations
            SET phase='exchanged' WHERE id='ra000000-0000-4000-8000-000000000001'",
    )
    .execute(&pool)
    .await;
    assert!(malformed_phase.is_err());
    pool.close().await;
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

#[tokio::test]
async fn enrollment_mode_and_lineage_constraints_reject_inconsistent_rows() -> Result<()> {
    // Review-2 line 17: SQLite must enforce the joint normal/recovery mode
    // shape and prevent a consumed recovery code from naming another host.
    let directory = std::env::temp_dir().join(format!("gridops-registry-mode-{}", Uuid::new_v4()));
    let pool = connect_database_path(&directory.join("registry.sqlite")).await?;
    sqlx::query(
        "INSERT INTO users
         (id,github_id,login,role,access_token,last_login_at,created_at,updated_at)
         VALUES ('mode-admin',900,'mode-admin','admin','',1,1,1)",
    )
    .execute(&pool)
    .await?;
    for (id, name) in [
        ("c0000000-0000-4000-8000-000000000001", "mode-a"),
        ("d0000000-0000-4000-8000-000000000001", "mode-b"),
    ] {
        sqlx::query(
            "INSERT INTO fleet_hosts (id,name,host_os,architecture,created_at,updated_at)
             VALUES (? ,?,'linux','x64',1,1)",
        )
        .bind(id)
        .bind(name)
        .execute(&pool)
        .await?;
    }
    let verifier = URL_SAFE_NO_PAD.encode([3_u8; 32]);
    let normal_with_host = sqlx::query(
        "INSERT INTO host_enrollments
         (id,code_verifier,intended_host_id,scope_json,issued_by,created_at,expires_at,
          mode,verifier_format,idempotency_key,request_hash)
         VALUES ('n0000000-0000-4000-8000-000000000001',?,?, '{}','mode-admin',1,1001,
                 'normal','sha256','mode-normal','mode-normal')",
    )
    .bind(verifier.clone())
    .bind("c0000000-0000-4000-8000-000000000001")
    .execute(&pool)
    .await;
    assert!(normal_with_host.is_err());
    let recovery_missing_expected = sqlx::query(
        "INSERT INTO host_enrollments
         (id,code_verifier,intended_host_id,scope_json,issued_by,created_at,expires_at,
          mode,verifier_format,idempotency_key,request_hash)
         VALUES ('n0000000-0000-4000-8000-000000000002',?,?, '{}','mode-admin',1,1001,
                 'recovery','sha256','mode-recovery-missing','mode-recovery-missing')",
    )
    .bind(URL_SAFE_NO_PAD.encode([4_u8; 32]))
    .bind("c0000000-0000-4000-8000-000000000001")
    .execute(&pool)
    .await;
    assert!(recovery_missing_expected.is_err());
    let recovery_wrong_consumed_host = sqlx::query(
        "INSERT INTO host_enrollments
         (id,code_verifier,intended_host_id,scope_json,issued_by,created_at,expires_at,
          consumed_at,consumed_host_id,mode,expected_host_epoch,expected_host_revision,
          verifier_format,idempotency_key,request_hash)
         VALUES ('n0000000-0000-4000-8000-000000000003',?,?, '{}','mode-admin',1,1001,
                 2,?,'recovery',1,1,'sha256','mode-recovery-lineage','mode-recovery-lineage')",
    )
    .bind(URL_SAFE_NO_PAD.encode([5_u8; 32]))
    .bind("c0000000-0000-4000-8000-000000000001")
    .bind("d0000000-0000-4000-8000-000000000001")
    .execute(&pool)
    .await;
    assert!(recovery_wrong_consumed_host.is_err());
    pool.close().await;
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

#[tokio::test]
async fn registry_identity_and_consumed_lineage_are_immutable_under_direct_sql() -> Result<()> {
    let directory =
        std::env::temp_dir().join(format!("gridops-registry-identity-{}", Uuid::new_v4()));
    let pool = connect_database_path(&directory.join("registry.sqlite")).await?;
    sqlx::query(
        "INSERT INTO users
         (id,github_id,login,role,access_token,last_login_at,created_at,updated_at)
         VALUES ('identity-admin',901,'identity-admin','admin','',1,1,1)",
    )
    .execute(&pool)
    .await?;
    let host_a = "e0000000-0000-4000-8000-000000000001";
    let host_b = "e0000000-0000-4000-8000-000000000002";
    for (id, name) in [(host_a, "identity-a"), (host_b, "identity-b")] {
        sqlx::query(
            "INSERT INTO fleet_hosts (id,name,host_os,architecture,created_at,updated_at)
             VALUES (? ,?,'linux','x64',1,1)",
        )
        .bind(id)
        .bind(name)
        .execute(&pool)
        .await?;
    }
    let verifier_a = URL_SAFE_NO_PAD.encode([11_u8; 32]);
    sqlx::query(
        "INSERT INTO host_credentials
         (id,host_id,host_epoch,generation,verifier,verifier_format,created_at)
         VALUES ('ea000000-0000-4000-8000-000000000001',?,1,1,?,'sha256',1)",
    )
    .bind(host_a)
    .bind(&verifier_a)
    .execute(&pool)
    .await?;
    let enrollment_id = "en000000-0000-4000-8000-000000000001";
    sqlx::query(
        "INSERT INTO host_enrollments
         (id,code_verifier,intended_host_id,scope_json,issued_by,created_at,expires_at,
          consumed_at,consumed_host_id,mode,verifier_format,idempotency_key,request_hash)
         VALUES (?,? ,NULL,'{}','identity-admin',1,1001,2,?,'normal','sha256','identity-lineage','identity-lineage')",
    )
    .bind(enrollment_id)
    .bind(URL_SAFE_NO_PAD.encode([12_u8; 32]))
    .bind(host_a)
    .execute(&pool)
    .await?;
    sqlx::query(
        "UPDATE fleet_hosts SET current_enrollment_id=?,current_enrollment_epoch=1 WHERE id=?",
    )
    .bind(enrollment_id)
    .bind(host_a)
    .execute(&pool)
    .await?;
    let issued_id = "en000000-0000-4000-8000-000000000002";
    sqlx::query(
        "INSERT INTO host_enrollments
         (id,code_verifier,intended_host_id,scope_json,issued_by,created_at,expires_at,
          mode,verifier_format,idempotency_key,request_hash)
         VALUES (?,? ,NULL,'{}','identity-admin',1,1001,'normal','sha256',?,?)",
    )
    .bind(issued_id)
    .bind(URL_SAFE_NO_PAD.encode([15_u8; 32]))
    .bind("identity-issued")
    .bind("identity-issued")
    .execute(&pool)
    .await?;

    let identity_id = "ea000000-0000-4000-8000-000000000099";
    assert!(
        sqlx::query("UPDATE host_credentials SET id=? WHERE id=?")
            .bind(identity_id)
            .bind("ea000000-0000-4000-8000-000000000001")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE host_credentials SET host_id=? WHERE id=?")
            .bind(host_b)
            .bind("ea000000-0000-4000-8000-000000000001")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE host_credentials SET host_epoch=2 WHERE id=?")
            .bind("ea000000-0000-4000-8000-000000000001")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE host_credentials SET generation=2 WHERE id=?")
            .bind("ea000000-0000-4000-8000-000000000001")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE host_credentials SET verifier=? WHERE id=?")
            .bind(URL_SAFE_NO_PAD.encode([13_u8; 32]))
            .bind("ea000000-0000-4000-8000-000000000001")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE host_credentials SET verifier_format='legacy' WHERE id=?")
            .bind("ea000000-0000-4000-8000-000000000001")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE host_credentials SET created_at=2 WHERE id=?")
            .bind("ea000000-0000-4000-8000-000000000001")
            .execute(&pool)
            .await
            .is_err()
    );
    let mut tx = pool.begin().await?;
    let allowed = sqlx::query(
        "UPDATE host_credentials SET overlap_expires_at=10
          WHERE id='ea000000-0000-4000-8000-000000000001'",
    )
    .execute(&mut *tx)
    .await?;
    assert_eq!(allowed.rows_affected(), 1);
    tx.rollback().await?;

    assert!(
        sqlx::query("UPDATE host_enrollments SET consumed_at=3 WHERE id=?")
            .bind(enrollment_id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE host_enrollments SET consumed_host_id=? WHERE id=?")
            .bind(host_b)
            .bind(enrollment_id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE host_enrollments SET mode='recovery' WHERE id=?")
            .bind(enrollment_id)
            .execute(&pool)
            .await
            .is_err()
    );
    for statement in [
        "UPDATE host_enrollments SET scope_json='{\"target_ids\":[]}' WHERE id=?",
        "UPDATE host_enrollments SET issued_by='other-admin' WHERE id=?",
        "UPDATE host_enrollments SET idempotency_key='rewritten' WHERE id=?",
        "UPDATE host_enrollments SET request_hash='rewritten' WHERE id=?",
        "UPDATE host_enrollments SET expires_at=2000 WHERE id=?",
        "UPDATE host_enrollments SET code_verifier=? WHERE id=?",
    ] {
        let query = if statement.contains("code_verifier") {
            sqlx::query(statement)
                .bind(URL_SAFE_NO_PAD.encode([14_u8; 32]))
                .bind(enrollment_id)
        } else {
            sqlx::query(statement).bind(enrollment_id)
        };
        assert!(query.execute(&pool).await.is_err(), "{statement}");
    }
    assert!(
        sqlx::query("UPDATE host_enrollments SET scope_json='{\"target_ids\":[]}' WHERE id=?")
            .bind(issued_id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE host_enrollments SET expires_at=2000 WHERE id=?")
            .bind(issued_id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query(
        "UPDATE fleet_hosts SET current_enrollment_id='en000000-0000-4000-8000-000000000099' WHERE id=?",
    )
    .bind(host_a)
    .execute(&pool)
    .await
    .is_err());
    assert!(
        sqlx::query("UPDATE fleet_hosts SET current_enrollment_epoch=2 WHERE id=?")
            .bind(host_a)
            .execute(&pool)
            .await
            .is_err()
    );
    let mut tx = pool.begin().await?;
    let allowed = sqlx::query("UPDATE host_enrollments SET revoked_at=3 WHERE id=?")
        .bind(enrollment_id)
        .execute(&mut *tx)
        .await?;
    assert_eq!(allowed.rows_affected(), 1);
    tx.rollback().await?;

    sqlx::query(
        "INSERT INTO host_credential_rotations
         (id,host_id,old_credential_id,old_generation,old_host_epoch,next_generation,idempotency_key,
          requested_by,request_method,request_resource,request_hash,phase,requested_at,expires_at)
         VALUES ('ra000000-0000-4000-8000-000000000001',?,'ea000000-0000-4000-8000-000000000001',1,1,2,
                 'rotation-identity','identity-admin','fleet.credential.request',?,
                 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd','requested',10,10010)",
    )
    .bind(host_a)
    .bind(host_a)
    .execute(&pool)
    .await?;
    for statement in [
        "UPDATE host_credential_rotations SET id='ra000000-0000-4000-8000-000000000002' WHERE id='ra000000-0000-4000-8000-000000000001'",
        "UPDATE host_credential_rotations SET old_generation=2 WHERE id='ra000000-0000-4000-8000-000000000001'",
        "UPDATE host_credential_rotations SET next_generation=3 WHERE id='ra000000-0000-4000-8000-000000000001'",
        "UPDATE host_credential_rotations SET requested_at=11 WHERE id='ra000000-0000-4000-8000-000000000001'",
        "UPDATE host_credential_rotations SET expires_at=20010 WHERE id='ra000000-0000-4000-8000-000000000001'",
    ] {
        assert!(
            sqlx::query(statement).execute(&pool).await.is_err(),
            "{statement}"
        );
    }

    pool.close().await;
    std::fs::remove_dir_all(directory)?;
    Ok(())
}
