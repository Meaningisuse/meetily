use super::types::{ClientBrief, ClientTokenOnce, Scope};
use crate::meeting::EnhancementError;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::sync::Arc;
use subtle::ConstantTimeEq;
pub trait Vault: Send + Sync {
    fn put(&self, id: &str, value: &str) -> Result<(), String>;
    fn get(&self, id: &str) -> Result<String, String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}
pub struct SystemVault;
impl Vault for SystemVault {
    fn put(&self, id: &str, value: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            keyring::Entry::new("com.meetily.internal-automation", id)
                .map_err(|_| "Keychain is unavailable")?
                .set_password(value)
                .map_err(|_| "Keychain write failed".into())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (id, value);
            Err("Secure credential storage is not supported on this platform".into())
        }
    }
    fn get(&self, id: &str) -> Result<String, String> {
        #[cfg(target_os = "macos")]
        {
            keyring::Entry::new("com.meetily.internal-automation", id)
                .map_err(|_| "Keychain is unavailable")?
                .get_password()
                .map_err(|_| "Keychain read failed".into())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = id;
            Err("Secure credential storage is not supported on this platform".into())
        }
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let entry = keyring::Entry::new("com.meetily.internal-automation", id)
                .map_err(|_| "Keychain is unavailable")?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err("Keychain deletion failed".into()),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = id;
            Err("Secure credential storage is not supported on this platform".into())
        }
    }
}
#[derive(Clone)]
pub struct ClientService {
    pool: SqlitePool,
    vault: Arc<dyn Vault>,
    writes: Arc<tokio::sync::Mutex<()>>,
}
impl ClientService {
    pub fn new(pool: SqlitePool, vault: Arc<dyn Vault>) -> Self {
        Self {
            pool,
            vault,
            writes: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
    pub async fn create(
        &self,
        name: &str,
        scopes: Vec<Scope>,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<ClientTokenOnce, EnhancementError> {
        if name.trim().is_empty()
            || name.chars().count() > 100
            || name.chars().any(char::is_control)
            || scopes != vec![Scope::Read]
        {
            return Err(EnhancementError::invalid(
                "Provide a client name and the currently supported read scope.",
            ));
        }
        let now = chrono::Utc::now().timestamp();
        let expires = expires_at.timestamp();
        if expires <= now || expires > now + 365 * 86400 {
            return Err(EnhancementError::invalid(
                "Expiry must be in the future within365days.",
            ));
        }
        let _writes = self.writes.lock().await;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM automation_clients WHERE revoked=0 AND expires_at>?",
        )
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        if count >= 128 {
            return Err(EnhancementError::new("BUSY", "Maximum128active clients."));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let mut random = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut random);
        let token: String = random.iter().map(|v| format!("{v:02x}")).collect();
        let hash = Sha256::digest(token.as_bytes());
        let vault = self.vault.clone();
        let secret_id = id.clone();
        let secret = token.clone();
        tokio::task::spawn_blocking(move || {
            if vault.put(&secret_id, &secret).is_err() {
                let _ = vault.delete(&secret_id);
                return Err("Keychain write failed".to_string());
            }
            match vault.get(&secret_id) {
                Ok(value) if value == secret => Ok(()),
                _ => {
                    let _ = vault.delete(&secret_id);
                    Err("Keychain verification failed".to_string())
                }
            }
        })
        .await
        .map_err(|_| EnhancementError::new("IO_ERROR", "Credential task failed"))?
        .map_err(|_| EnhancementError::new("IO_ERROR", "Secure credential could not be saved"))?;
        let result=async {sqlx::query("INSERT INTO automation_clients(id,name,token_hash,scopes_json,secret_ref,expires_at,created_at) VALUES(?,?,?,?,?,?,?)").bind(&id).bind(name.trim()).bind(hash.as_slice()).bind(serde_json::to_string(&scopes).unwrap()).bind(&id).bind(expires).bind(now).execute(&mut *tx).await?;tx.commit().await}.await;
        if let Err(error) = result {
            let vault = self.vault.clone();
            let secret_id = id.clone();
            let _ = tokio::task::spawn_blocking(move || vault.delete(&secret_id)).await;
            return Err(error.into());
        }
        Ok(ClientTokenOnce {
            client: ClientBrief {
                id,
                name: name.trim().into(),
                scopes,
                expires_at: expires,
                revoked: false,
            },
            token,
        })
    }
    pub async fn verify(
        &self,
        token: &str,
        required: Scope,
    ) -> Result<ClientBrief, EnhancementError> {
        if token.len() != 64 || !token.bytes().all(|v| v.is_ascii_hexdigit()) {
            return Err(EnhancementError::new(
                "UNAUTHORIZED",
                "Invalid or expired credentials",
            ));
        }
        let hash = Sha256::digest(token.as_bytes());
        let row:Option<(String,String,Vec<u8>,String,i64,bool)>=sqlx::query_as("SELECT id,name,token_hash,scopes_json,expires_at,revoked FROM automation_clients WHERE token_hash=?").bind(hash.as_slice()).fetch_optional(&self.pool).await?;
        let dummy = [0u8; 32];
        let expected = row.as_ref().map(|r| r.2.as_slice()).unwrap_or(&dummy);
        let matches = hash.as_slice().ct_eq(expected).unwrap_u8() == 1;
        let Some((id, name, _, scopes, expires_at, revoked)) = row else {
            return Err(EnhancementError::new(
                "UNAUTHORIZED",
                "Invalid or expired credentials",
            ));
        };
        if !matches || revoked || expires_at <= chrono::Utc::now().timestamp() {
            return Err(EnhancementError::new(
                "UNAUTHORIZED",
                "Invalid or expired credentials",
            ));
        }
        let scopes: Vec<Scope> = serde_json::from_str(&scopes)
            .map_err(|_| EnhancementError::new("IO_ERROR", "Invalid credential scopes"))?;
        if !scopes.contains(&required) {
            return Err(EnhancementError::new(
                "FORBIDDEN",
                "This client does not have the required permission",
            ));
        }
        Ok(ClientBrief {
            id,
            name,
            scopes,
            expires_at,
            revoked,
        })
    }
    pub async fn list(&self) -> Result<Vec<ClientBrief>, EnhancementError> {
        let rows:Vec<(String,String,String,i64,bool)>=sqlx::query_as("SELECT id,name,scopes_json,expires_at,revoked FROM automation_clients ORDER BY created_at DESC,id LIMIT 128").fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|(id, name, scopes, expires_at, revoked)| {
                Ok(ClientBrief {
                    id,
                    name,
                    scopes: serde_json::from_str(&scopes).map_err(|_| {
                        EnhancementError::new("IO_ERROR", "Invalid credential scopes")
                    })?,
                    expires_at,
                    revoked,
                })
            })
            .collect()
    }
    pub async fn revoke(&self, id: &str) -> Result<(), EnhancementError> {
        let _writes = self.writes.lock().await;
        let updated = sqlx::query("UPDATE automation_clients SET revoked=1 WHERE id=?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if updated.rows_affected() == 0 {
            return Err(EnhancementError::new("NOT_FOUND", "Client not found"));
        }
        let vault = self.vault.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || vault.delete(&id))
            .await
            .map_err(|_| EnhancementError::new("IO_ERROR", "Credential cleanup failed"))?
            .map_err(|_| {
                EnhancementError::new("IO_ERROR", "Client revoked but Keychain cleanup failed")
            })?;
        Ok(())
    }
}
