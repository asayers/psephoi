use crate::{AppState, ElectionState, STATE_DIR, vote::read_votes};
use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::Response,
};
use jiff::Timestamp;
use psephoi::{CandidateResults, Election, ElectionResults};
use rustix::fs::{AtFlags, OFlags, linkat, open};
use std::{
    fs::{File, Permissions},
    io::{ErrorKind, Write},
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

pub fn atomic_write(dir: &std::path::Path, fname: &str, contents: &[u8]) -> Result<()> {
    let perms = Permissions::from_mode(0o666);
    let mut file: File = open(dir, OFlags::TMPFILE | OFlags::RDWR, perms.mode().into())?.into();
    file.write_all(contents)?;
    file.sync_all()?;
    match linkat(
        file,
        "",
        rustix::fs::CWD,
        dir.join(fname),
        AtFlags::EMPTY_PATH,
    ) {
        Ok(()) => (),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => (), // contents are deterministic/immutable, so this is fine
        Err(e) => Err(e)?,
    }
    Ok(())
}

impl ElectionState {
    fn results(&mut self) -> Result<ElectionResults, (StatusCode, String)> {
        if Timestamp::now() < self.config.voting_period.end {
            return Err((StatusCode::FORBIDDEN, "Voting is not yet closed".into()));
        }
        if let Some(res) = self.results.clone() {
            Ok(res)
        } else {
            let results = self
                .compute_results()
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
            Ok(results)
        }
    }

    fn compute_results(&mut self) -> Result<ElectionResults> {
        let xs = read_votes(&self.dir, &self.config)?;
        let candidates = xs
            .into_iter()
            .enumerate()
            .map(|(i, psephoi)| {
                let tally = psephoi.len() as u32;
                let nonces = psephoi.keys().copied().flatten().collect::<Vec<u8>>();
                let nonces_hash = blake3::hash(&nonces);
                atomic_write(&self.dir, &format!("nonces_{i}.bin"), &nonces)?;
                let sigs = psephoi.values().copied().flatten().collect::<Vec<u8>>();
                let sigs_hash = blake3::hash(&sigs);
                atomic_write(&self.dir, &format!("sigs_{i}.bin"), &sigs)?;
                Ok(CandidateResults {
                    tally,
                    nonces: *nonces_hash.as_bytes(),
                    sigs: *sigs_hash.as_bytes(),
                })
            })
            .collect::<Result<_>>()?;
        // TODO: Discard the WAL

        // TODO: Sign it
        let results = ElectionResults {
            election: *self.hash.0.as_bytes(),
            candidates,
        };
        atomic_write(&self.dir, "results.bin", &results.to_bytes())?;
        self.results = Some(results.clone());
        self.vote_wals.clear(); // Ensure we're not still writing to the WALs
        Ok(results)
    }
}

pub async fn results_get(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(election): Path<Election>,
) -> Result<Vec<u8>, (StatusCode, String)> {
    let mut state = state.lock().unwrap();
    let state = state.load_election_http(election)?;
    match state.results() {
        Ok(resp) => Ok(resp.to_bytes()),
        Err((code, error)) => Err((code, error)),
    }
}

pub async fn manifest_get(
    Path(election): Path<Election>,
) -> Result<Response, (StatusCode, String)> {
    static_get(election, "manifest.bin")
}

pub async fn nonces_get(
    Path((election, i)): Path<(Election, u32)>,
) -> Result<Response, (StatusCode, String)> {
    static_get(election, &format!("nonces_{i}.bin"))
}

pub async fn sigs_get(
    Path((election, i)): Path<(Election, u32)>,
) -> Result<Response, (StatusCode, String)> {
    static_get(election, &format!("sigs_{i}.bin"))
}

/// Files from the election dir which should be publically accessible.  These
/// files may or not exist, depending on what phase the election is in. Once
/// written, though, these files are immutable.  So even though these URLs are
/// not content-addressed, the fact that they include the election ID is enough
/// to uniquely pin their contents (once they exist).
pub fn static_get(election: Election, fname: &str) -> Result<Response, (StatusCode, String)> {
    // TODO: Require fs-verity?
    let path = STATE_DIR.join(election.to_string()).join(fname);
    let data = std::fs::read(path).map_err(|e| (StatusCode::NOT_FOUND, format!("{e:#}")))?;
    Ok(Response::builder()
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(data.into())
        .unwrap())
}
