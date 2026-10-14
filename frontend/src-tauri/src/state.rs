use crate::{database::manager::DatabaseManager, jobs::JobService};

pub struct AppState {
    pub db_manager: DatabaseManager,
    pub jobs: JobService,
    pub automation: crate::automation::server::AutomationServer,
}
impl AppState {
    pub async fn new(db_manager: DatabaseManager, app: tauri::AppHandle) -> Result<Self, String> {
        let jobs = JobService::new(db_manager.pool().clone());
        jobs.recover_interrupted()
            .await
            .map_err(|error| error.to_string())?;
        let clients = crate::automation::auth::ClientService::new(
            db_manager.pool().clone(),
            std::sync::Arc::new(crate::automation::auth::SystemVault),
        );
        let app_data = db_manager
            .pool()
            .connect_options()
            .get_filename()
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let automation = crate::automation::server::AutomationServer::new(
            db_manager.pool().clone(),
            clients,
            app_data,
        );
        automation.attach_app_handle(app).await;
        if let Err(error) = automation.restore_preference().await {
            log::error!("Automation remained disabled: {error}");
        }
        Ok(Self {
            db_manager,
            jobs,
            automation,
        })
    }
}
