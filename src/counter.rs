use crate::announcer::Election;
use axum::extract::Path;

pub async fn vote_root() -> &'static str {
    "Hello from the ballot counter!"
}
pub async fn vote_post(Path(election): Path<Election>) -> String {
    format!("ballot counter for {election}")
}
