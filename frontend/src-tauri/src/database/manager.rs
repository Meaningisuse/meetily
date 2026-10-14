use super::recovery::RecoveryService;
use sqlx::{migrate::MigrateDatabase, Result, Sqlite, SqlitePool, Transaction};
use std::fs;
use std::path::Path;
use tauri::Manager;

#[derive(Clone)]
pub struct DatabaseManager {
    pool: SqlitePool,
}

impl DatabaseManager {
    pub async fn new(tauri_db_path: &str, backend_db_path: &str) -> Result<Self> {
        if let Some(parent_dir) = Path::new(tauri_db_path).parent() {
            if !parent_dir.exists() {
                fs::create_dir_all(parent_dir).map_err(|e| sqlx::Error::Io(e))?;
            }
        }

        let db_path=Path::new(tauri_db_path);
        let existing=db_path.exists() || Path::new(backend_db_path).exists();
        if db_path.exists() && Path::new(&format!("{tauri_db_path}-wal")).exists() {
            let backup=RecoveryService::backup_before_recovery(db_path).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
            log::info!("Preserved startup DB/WAL/SHM evidence at {}",backup.display());
        }
        if !db_path.exists() {
            if Path::new(backend_db_path).exists() {
                log::info!(
                    "Copying database from {} to {}",
                    backend_db_path,
                    tauri_db_path
                );
                RecoveryService::copy_online_path(Path::new(backend_db_path),db_path).await.map_err(|e|sqlx::Error::Protocol(e.to_string()))?;
            } else {
                log::info!("Creating database at {}", tauri_db_path);
                Sqlite::create_database(tauri_db_path).await?;
            }
        }

        let pool = SqlitePool::connect(tauri_db_path).await?;

        let migration_result=async {
            let migrator=sqlx::migrate!("./migrations");
            let tracked:i64=sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='_sqlx_migrations'").fetch_one(&pool).await?;
            let applied:Vec<i64>=if tracked>0 {sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success=1").fetch_all(&pool).await?} else {Vec::new()};
            if existing && migrator.iter().any(|m| !applied.contains(&m.version)) {
                let backup=RecoveryService::migration_backup(&pool,db_path).await.map_err(|e|sqlx::Error::Protocol(e.to_string()))?;
                log::info!("Pre-migration consistent backup: {}",backup.display());
            }
            migrator.run(&pool).await.map_err(sqlx::Error::from)
        }.await;
        if let Err(error)=migration_result { pool.close().await; return Err(error); }

        Ok(DatabaseManager { pool })
    }

    // NOTE: So for the first time users they needs to start the application
    // after they can just delete the existing .sqlite file and then copy the existing .db file to
    // the current app dir, So the system detects legacy db and copy it and starts with that data
    // (Newly created .sqlite with the copied content from .db)
    pub async fn new_from_app_handle(app_handle: &tauri::AppHandle) -> Result<Self> {
        // Resolve the app's data directory
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");
        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(|e| sqlx::Error::Io(e))?;
        }

        // Define database paths
        let tauri_db_path = app_data_dir
            .join("meeting_minutes.sqlite")
            .to_string_lossy()
            .to_string();
        // Legacy backend DB path (for auto-migration if exists)
        let backend_db_path = app_data_dir
            .join("meeting_minutes.db")
            .to_string_lossy()
            .to_string();

        match Self::new(&tauri_db_path,&backend_db_path).await {
            Ok(manager)=>Ok(manager),
            Err(error)=> {
                let message=error.to_string();
                if message.contains("malformed") || message.contains("corrupt") {
                    // All application connections from the failed attempt have
                    // been closed. Do not discard a possibly committed WAL.
                    let backup=RecoveryService::backup_before_recovery(Path::new(&tauri_db_path)).map_err(|e|sqlx::Error::Protocol(e.to_string()))?;
                    log::error!("Database needs recovery. Original DB/WAL/SHM retained; evidence: {}",backup.display());
                    return Err(sqlx::Error::Protocol(format!("Database recovery is required. Original files were retained; recovery materials: {}",backup.display())));
                }
                Err(error)
            }
        }
    }

    /// Check if this is the first launch (sqlite database doesn't exist yet)
    pub async fn is_first_launch(app_handle: &tauri::AppHandle) -> Result<bool> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");

        let tauri_db_path = app_data_dir.join("meeting_minutes.sqlite");

        Ok(!tauri_db_path.exists())
    }

    /// Import a legacy database from the specified path and initialize
    pub async fn import_legacy_database(
        app_handle: &tauri::AppHandle,
        legacy_db_path: &str,
    ) -> Result<Self> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");

        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(|e| sqlx::Error::Io(e))?;
        }

        // Copy legacy database to app data directory as meeting_minutes.db
        let target_legacy_path = app_data_dir.join("meeting_minutes.db");
        log::info!(
            "Copying legacy database from {} to {}",
            legacy_db_path,
            target_legacy_path.display()
        );

        RecoveryService::copy_online_path(Path::new(legacy_db_path),&target_legacy_path).await.map_err(|e|sqlx::Error::Protocol(e.to_string()))?;

        // Now use the standard initialization which will detect and migrate the legacy db
        Self::new_from_app_handle(app_handle).await
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn with_transaction<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Transaction<'_, Sqlite>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut tx = self.pool.begin().await?;
        let result = f(&mut tx).await;

        match result {
            Ok(val) => {
                tx.commit().await?;
                Ok(val)
            }
            Err(err) => {
                tx.rollback().await?;
                Err(err)
            }
        }
    }

    /// Cleanup database connection and checkpoint WAL
    /// This should be called on application shutdown to ensure:
    /// - All WAL changes are written to the main database file
    /// - The .wal and .shm files are deleted
    /// - Connection pool is gracefully closed
    pub async fn cleanup(&self) -> Result<()> {
        log::info!("Starting database cleanup...");

        // Force checkpoint of WAL to main database file and remove WAL file
        // TRUNCATE mode: checkpoints all pages AND deletes the WAL file
        match sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await
        {
            Ok(_) => log::info!("WAL checkpoint completed successfully"),
            Err(e) => log::warn!("WAL checkpoint failed (non-fatal): {}", e),
        }

        // Close the connection pool gracefully
        self.pool.close().await;
        log::info!("Database connection pool closed");

        Ok(())
    }
}
