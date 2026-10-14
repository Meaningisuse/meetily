pub mod commands;
pub mod manager;
pub mod recovery;
pub mod models;
pub mod repositories;
pub mod setup;

#[cfg(test)]
mod recovery_tests {
    use super::recovery::RecoveryService;
    use std::fs;

    #[test]
    fn recovery_preserves_wal_before_mutation() {
        let directory=tempfile::tempdir().unwrap();
        let db=directory.path().join("meeting.sqlite");
        fs::write(&db,b"original database bytes").unwrap();
        fs::write(directory.path().join("meeting.sqlite-wal"),b"uncheckpointed meeting").unwrap();
        fs::write(directory.path().join("meeting.sqlite-shm"),b"shared memory").unwrap();
        let backup=RecoveryService::backup_before_recovery(&db).unwrap();
        assert_eq!(fs::read(backup.join("meeting.sqlite-wal")).unwrap(),b"uncheckpointed meeting");
        assert_eq!(fs::read(backup.join("meeting.sqlite")).unwrap(),b"original database bytes");
        assert!(backup.join("manifest.json").exists());
        assert_eq!(fs::read(directory.path().join("meeting.sqlite-wal")).unwrap(),b"uncheckpointed meeting");
        // Simulate a backup target that cannot be a directory. Originals must survive.
        let blocked=directory.path().join("blocked"); fs::write(&blocked,b"not a directory").unwrap();
        assert!(RecoveryService::backup_to(&db,&blocked).is_err());
        assert!(db.exists()); assert!(directory.path().join("meeting.sqlite-wal").exists());
    }

    #[tokio::test]
    async fn online_backup_includes_committed_wal_and_does_not_overwrite() {
        let directory=tempfile::tempdir().unwrap();
        let db=directory.path().join("live.sqlite");
        let options=sqlx::sqlite::SqliteConnectOptions::new().filename(&db).create_if_missing(true).journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(2).connect_with(options).await.unwrap();
        sqlx::query("PRAGMA wal_autocheckpoint=0").execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE evidence(id INTEGER PRIMARY KEY,value TEXT)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO evidence(value) VALUES('committed in WAL')").execute(&pool).await.unwrap();
        assert!(directory.path().join("live.sqlite-wal").exists());
        let backup=directory.path().join("snapshot.sqlite");
        RecoveryService::online_backup(&pool,&backup).await.unwrap();
        let copied=sqlx::sqlite::SqlitePoolOptions::new().connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&backup).read_only(true)).await.unwrap();
        let value:String=sqlx::query_scalar("SELECT value FROM evidence").fetch_one(&copied).await.unwrap();
        assert_eq!(value,"committed in WAL"); copied.close().await;
        assert!(RecoveryService::online_backup(&pool,&backup).await.is_err());
        let original:String=sqlx::query_scalar("SELECT value FROM evidence").fetch_one(&pool).await.unwrap();
        assert_eq!(original,value);
        pool.close().await;
    }
}

#[cfg(test)]
mod manager_backup_tests {
    #[tokio::test]
    async fn migration_takes_readable_backup_before_new_schema() {
        let directory=tempfile::tempdir().unwrap(); let db=directory.path().join("upgrade.sqlite");
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&db).create_if_missing(true)).await.unwrap();
        let all=sqlx::migrate!("./migrations");
        let old=sqlx::migrate::Migrator { migrations: std::borrow::Cow::Owned(all.iter().take(10).cloned().collect()), ..sqlx::migrate::Migrator::DEFAULT };
        old.run(&pool).await.unwrap();
        crate::meeting::tests::seed(&pool,"before-upgrade").await; pool.close().await;
        let manager=super::manager::DatabaseManager::new(db.to_str().unwrap(),directory.path().join("none.db").to_str().unwrap()).await.unwrap();
        let backups:Vec<_>=std::fs::read_dir(directory.path().join("recovery-backups")).unwrap().map(|entry|entry.unwrap().path()).filter(|path|path.extension().is_some_and(|e|e=="sqlite")).collect();
        assert_eq!(backups.len(),1);
        let copied=sqlx::sqlite::SqlitePoolOptions::new().connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&backups[0]).read_only(true)).await.unwrap();
        let title:String=sqlx::query_scalar("SELECT title FROM meetings WHERE id='before-upgrade'").fetch_one(&copied).await.unwrap(); assert_eq!(title,"中文会议");
        let new_table:i64=sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='meeting_content_versions'").fetch_one(&copied).await.unwrap(); assert_eq!(new_table,0);
        copied.close().await; manager.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn migration_backup_failure_keeps_original_schema_and_wal() {
        let directory=tempfile::tempdir().unwrap(); let db=directory.path().join("blocked.sqlite");
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&db).create_if_missing(true)).await.unwrap();
        let all=sqlx::migrate!("./migrations");
        let old=sqlx::migrate::Migrator { migrations: std::borrow::Cow::Owned(all.iter().take(10).cloned().collect()), ..sqlx::migrate::Migrator::DEFAULT };
        old.run(&pool).await.unwrap(); crate::meeting::tests::seed(&pool,"preserved").await; pool.close().await;
        std::fs::write(directory.path().join("recovery-backups"),b"backup location blocked").unwrap();
        assert!(super::manager::DatabaseManager::new(db.to_str().unwrap(),"nonexistent.db").await.is_err());
        let check=sqlx::sqlite::SqlitePoolOptions::new().connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&db).read_only(true)).await.unwrap();
        let new_table:i64=sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='meeting_content_versions'").fetch_one(&check).await.unwrap(); assert_eq!(new_table,0);
        let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM meetings WHERE id='preserved'").fetch_one(&check).await.unwrap(); assert_eq!(count,1); check.close().await;
    }
}

#[cfg(test)]
mod legacy_backup_tests {
    #[tokio::test]
    async fn importing_live_legacy_database_includes_its_wal() {
        let directory=tempfile::tempdir().unwrap(); let source=directory.path().join("legacy.db");
        let pool=sqlx::sqlite::SqlitePoolOptions::new().connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&source).create_if_missing(true).journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)).await.unwrap();
        sqlx::query("CREATE TABLE retained(value TEXT)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO retained VALUES('last committed meeting')").execute(&pool).await.unwrap();
        let destination=directory.path().join("import.sqlite");
        super::recovery::RecoveryService::copy_online_path(&source,&destination).await.unwrap();
        let copied=sqlx::sqlite::SqlitePoolOptions::new().connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&destination).read_only(true)).await.unwrap();
        let text:String=sqlx::query_scalar("SELECT value FROM retained").fetch_one(&copied).await.unwrap(); assert_eq!(text,"last committed meeting"); copied.close().await; pool.close().await;
    }
}
