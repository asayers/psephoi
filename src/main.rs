mod announcer;
mod counter;
mod registrar;

use crate::announcer::*;
use crate::counter::*;
use crate::registrar::*;
use anyhow::Result;
use axum::{
    Router,
    routing::{get, post},
};
use core::fmt;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Default)]
pub struct AppState {
    registrar: RegistrarState,
    announcer: AnnouncerState,
}

impl AppState {
    fn insert(&mut self, state: ElectionState) {
        let manifest = state.manifest();
        let bytes = manifest.bytes();
        let hash = manifest.hash();
        self.announcer.manifests.insert(hash, bytes);
        self.registrar.elections.insert(hash, state);
    }

    pub fn dummy() -> Result<AppState> {
        let mut state = AppState::default();
        let mut foo = ElectionState::new("general election 2026".to_owned());
        foo.candidates
            .insert(Candidate("labour".to_owned()), CandidateState::new()?);
        foo.candidates
            .insert(Candidate("conservative".to_owned()), CandidateState::new()?);
        state.insert(foo);
        Ok(state)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Ultimately these will be 3 different servers at 3 different domains: the
    // registrar (API only), the ballot-counter (API only), and the user-facing
    // one which serves the javascript etc..  For now it's just endpoints on the
    // same server to keep things simple.
    let app = Router::new()
        .route("/", get(list_elections))
        .route("/registrar/{election}", post(registrar_post))
        .route("/vote/", get(vote_root))
        .route("/vote/{election}", post(vote_post))
        .route("/manifest/{election}", get(get_manifest))
        .with_state(Arc::new(Mutex::new(AppState::dummy()?)));

    let sock = TcpListener::bind("0.0.0.0:3000").await?;
    Ok(axum::serve(sock, app).await?)
}

// async fn get_election(
//     State(state): State<Arc<Mutex<AppState>>>,
//     Path(election): Path<Election>,
// ) -> (StatusCode, String) {
//     let state = state.lock().unwrap();
//     let Some(state) = state.elections.get(&election) else {
//         return (
//             StatusCode::NOT_FOUND,
//             format!("{election}: No such election"),
//         );
//     };
//     (StatusCode::OK, format!("{state:?}"))
// }

// #[derive(TlsSerialize, TlsDeserialize, TlsSize)]
// struct PrivacyPassToken {
//     token_type: u16, // == 0x0002
//     nonce: [u8; 32],
//     challenge_digest: [u8; 32],
//     token_key_id: [u8; 32],
//     authenticator: [u8; Nk],
// }

#[derive(Clone, PartialEq, Eq, Hash, Debug, Deserialize)]
struct Candidate(String);
impl fmt::Display for Candidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
