use crate::{database::manager::DatabaseManager, jobs::JobService};

pub struct AppState {
    pub db_manager: DatabaseManager,
    pub jobs: JobService,
}
impl AppState {
    pub async fn new(db_manager: DatabaseManager) -> Result<Self, String> {
        let jobs = JobService::new(db_manager.pool().clone());
        jobs.recover_interrupted()
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self { db_manager, jobs })
    }
}
