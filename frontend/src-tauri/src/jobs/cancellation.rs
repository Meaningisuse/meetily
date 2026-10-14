use tokio_util::sync::CancellationToken;
#[derive(Clone)]
pub struct JobContext {
    pub job_id: String,
    pub cancellation: CancellationToken,
}
