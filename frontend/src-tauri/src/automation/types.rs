use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Read,
    Write,
    Record,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    pub pid: u32,
    pub instance_id: String,
    pub base_url: String,
    pub protocol_version: u8,
}
#[derive(Debug, Clone, Serialize)]
pub struct ClientBrief {
    pub id: String,
    pub name: String,
    pub scopes: Vec<Scope>,
    pub expires_at: i64,
    pub revoked: bool,
}
// Deliberately not Debug: contains the one-time credential.
#[derive(Serialize)]
pub struct ClientTokenOnce {
    pub client: ClientBrief,
    pub token: String,
}
