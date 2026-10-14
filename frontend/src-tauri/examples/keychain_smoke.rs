//! Exercise a fresh, owned credential only; no existing items are read.
use app_lib::automation::auth::{SystemVault,Vault};
fn main()->anyhow::Result<()> {
 #[cfg(not(target_os="macos"))] anyhow::bail!("This probe requires the macOS Keychain");
 let vault=SystemVault;let id=format!("smoke-{}",uuid::Uuid::new_v4());
 let value=format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple());
 let check=(||->anyhow::Result<()> {vault.put(&id,&value).map_err(anyhow::Error::msg)?;let read=vault.get(&id).map_err(anyhow::Error::msg)?;if read!=value {anyhow::bail!("Credential readback mismatch");}Ok(())})();
 let cleanup=vault.delete(&id).map_err(anyhow::Error::msg);check?;cleanup?;println!("owned_keychain_write_readback_delete=passed");Ok(())
}
