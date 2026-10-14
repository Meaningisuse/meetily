use super::{
    auth::{ClientService, Vault},
    server::AutomationServer,
    types::Scope,
};
use crate::meeting::tests::pool;
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct TestVault {
    secrets: Mutex<std::collections::HashMap<String, String>>,
    fail: bool,
    read_fail: bool,
}
impl Vault for TestVault {
    fn put(&self, id: &str, value: &str) -> Result<(), String> {
        if self.fail {
            return Err("vault unavailable".into());
        }
        self.secrets.lock().unwrap().insert(id.into(), value.into());
        Ok(())
    }
    fn get(&self, id: &str) -> Result<String, String> {
        if self.read_fail {
            return Err("readback unavailable".into());
        }
        self.secrets
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("missing".into())
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.secrets.lock().unwrap().remove(id);
        Ok(())
    }
}
#[tokio::test]
async fn disabled_by_default_and_loopback_only() {
    let folder = tempfile::tempdir().unwrap();
    let pool = pool().await;
    let clients = ClientService::new(pool.clone(), Arc::new(TestVault::default()));
    let server = AutomationServer::new(pool, clients, folder.path().to_path_buf());
    assert!(server.status().await.is_none());
    assert!(!folder.path().join("automation/discovery.json").exists());
    let first = server.start().await.unwrap();
    assert!(first.base_url.starts_with("http://127.0.0.1:"));
    assert_eq!(server.start().await.unwrap().instance_id, first.instance_id);
    let discovery = folder.path().join("automation/discovery.json");
    assert!(discovery.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&discovery).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(discovery.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    server.stop().await.unwrap();
    assert!(!discovery.exists());
    assert!(reqwest::get(format!("{}/health", first.base_url))
        .await
        .is_err());
    std::fs::write(&discovery, b"stale pid/instance").unwrap();
    let second = server.start().await.unwrap();
    assert_ne!(first.instance_id, second.instance_id);
    server.stop().await.unwrap();
}
#[tokio::test]
async fn auth_revocation_origin_and_host() {
    let folder = tempfile::tempdir().unwrap();
    let pool = pool().await;
    let vault = Arc::new(TestVault::default());
    let clients = ClientService::new(pool.clone(), vault.clone());
    let token = clients
        .create(
            "internal reader",
            vec![Scope::Read],
            chrono::Utc::now() + chrono::Duration::days(7),
        )
        .await
        .unwrap();
    assert_eq!(token.token.len(), 64);
    let server = AutomationServer::new(pool.clone(), clients.clone(), folder.path().to_path_buf());
    let discovery = server.start().await.unwrap();
    let http = reqwest::Client::new();
    assert_eq!(
        http.get(format!("{}/meetings", discovery.base_url))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.get(format!("{}/health", discovery.base_url))
            .header("Origin", "https://evil.test")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.get(format!("{}/health", discovery.base_url))
            .header("Host", "attacker.test")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let health: serde_json::Value = http
        .get(format!("{}/health", discovery.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["instance_id"], discovery.instance_id);
    assert!(!health.to_string().contains("export"));
    let file = std::fs::read_to_string(folder.path().join("automation/discovery.json")).unwrap();
    assert!(!file.contains(&token.token));
    clients.verify(&token.token, Scope::Read).await.unwrap();
    assert_eq!(
        clients
            .verify(&token.token, Scope::Record)
            .await
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );
    sqlx::query("UPDATE automation_clients SET expires_at=0 WHERE id=?")
        .bind(&token.client.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        clients
            .verify(&token.token, Scope::Read)
            .await
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    sqlx::query("UPDATE automation_clients SET expires_at=? WHERE id=?")
        .bind(chrono::Utc::now().timestamp() + 3600)
        .bind(&token.client.id)
        .execute(&pool)
        .await
        .unwrap();
    clients.revoke(&token.client.id).await.unwrap();
    assert_eq!(
        clients
            .verify(&token.token, Scope::Read)
            .await
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    assert!(vault.secrets.lock().unwrap().is_empty());
    server.stop().await.unwrap();
}
#[tokio::test]
async fn credential_failure_never_creates_a_client() {
    let pool = pool().await;
    let clients = ClientService::new(
        pool.clone(),
        Arc::new(TestVault {
            fail: true,
            ..Default::default()
        }),
    );
    assert!(clients
        .create(
            "fail",
            vec![Scope::Read],
            chrono::Utc::now() + chrono::Duration::days(1)
        )
        .await
        .is_err());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automation_clients")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn failed_credential_readback_cleans_the_orphan_secret() {
    let pool = pool().await;
    let vault = Arc::new(TestVault {
        read_fail: true,
        ..Default::default()
    });
    let clients = ClientService::new(pool.clone(), vault.clone());
    assert!(clients
        .create(
            "readback",
            vec![Scope::Read],
            chrono::Utc::now() + chrono::Duration::days(1)
        )
        .await
        .is_err());
    assert!(vault.secrets.lock().unwrap().is_empty());
}
#[tokio::test]
async fn rate_body_limits_and_other_instances_are_rejected() {
    let folder = tempfile::tempdir().unwrap();
    let pool = pool().await;
    let clients = ClientService::new(pool.clone(), Arc::new(TestVault::default()));
    let token = clients
        .create(
            "limits",
            vec![Scope::Read],
            chrono::Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
    let server = AutomationServer::new(pool.clone(), clients.clone(), folder.path().to_path_buf());
    let discovery = server.start().await.unwrap();
    let other = AutomationServer::new(pool, clients, folder.path().to_path_buf());
    assert_eq!(other.start().await.unwrap_err().code, "BUSY");
    let http = reqwest::Client::new();
    let large = http
        .get(format!("{}/unregistered-endpoint", discovery.base_url))
        .bearer_auth(&token.token)
        .body(vec![0u8; 1024 * 1024 + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(large.status(), 422);
    for _ in 0..59 {
        let reply = http
            .get(format!("{}/unregistered-endpoint", discovery.base_url))
            .bearer_auth(&token.token)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), 404);
    }
    assert_eq!(
        http.get(format!("{}/unregistered-endpoint", discovery.base_url))
            .bearer_auth(&token.token)
            .send()
            .await
            .unwrap()
            .status(),
        429
    );
    server.stop().await.unwrap();
}

#[tokio::test]
async fn only_explicit_enable_is_restored_after_restart() {
    let folder = tempfile::tempdir().unwrap();
    let pool = pool().await;
    let clients = ClientService::new(pool.clone(), Arc::new(TestVault::default()));
    let server = AutomationServer::new(pool.clone(), clients.clone(), folder.path().to_path_buf());
    server.restore_preference().await.unwrap();
    assert!(server.status().await.is_none());
    server.set_enabled(true).await.unwrap();
    let first = server.status().await.unwrap();
    server.stop().await.unwrap();
    let restarted = AutomationServer::new(pool, clients, folder.path().to_path_buf());
    restarted.restore_preference().await.unwrap();
    assert_ne!(
        first.instance_id,
        restarted.status().await.unwrap().instance_id
    );
    restarted.set_enabled(false).await.unwrap();
    restarted.restore_preference().await.unwrap();
    assert!(restarted.status().await.is_none());
}
