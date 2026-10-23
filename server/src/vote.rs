use crate::{AppState, ElectionState, election::ElectionConfig};
use anyhow::{Result, anyhow};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
};
use jiff::Timestamp;
use log::warn;
use psephoi::{Election, NONCE_LEN, Psephos, SIG_LEN, VoteRequest, VoteResponse};
use rustix::fs::{FallocateFlags, fallocate, ftruncate};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    sync::{Arc, Mutex},
};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub fn read_votes(
    dir: &std::path::Path,
    config: &ElectionConfig,
) -> Result<Vec<BTreeMap<[u8; NONCE_LEN], [u8; SIG_LEN]>>> {
    let mut xs = vec![];
    for i in 0..config.candidates.len() {
        let path = dir.join(format!("votes_{i}.wal"));
        let wal_mmap = mmap_snapshot::Mmap::open(&path)?;
        let (wal, suffix) =
            <[VoteWalItem]>::ref_from_prefix(&wal_mmap).map_err(|e| anyhow!("Corrupt WAL: {e}"))?;
        let ps: BTreeMap<_, _> = wal
            .iter()
            // TODO: Re-verify sig here, to catch corruption
            .map(|i| (i.psephos.token, i.psephos.sig))
            .collect();
        let new_len = (!suffix.is_empty()).then_some(wal_mmap.len() - suffix.len());
        std::mem::drop(wal_mmap);
        let f = File::options().write(true).open(&path)?;
        if let Some(new_len) = new_len {
            warn!("Discarding partial write at end of WAL");
            ftruncate(&f, new_len as u64)?;
        }
        let size = std::mem::size_of::<VoteWalItem>() * config.n_voters as usize;
        fallocate(&f, FallocateFlags::KEEP_SIZE, 0, size as u64)?;
        xs.push(ps);
    }
    Ok(xs)
}

#[derive(KnownLayout, FromBytes, IntoBytes, Immutable)]
#[repr(C)]
pub struct VoteWalItem {
    // TODO: checksum?
    psephos: Psephos,
    padding: [u8; 240], // Zeroes
}

// Avoid straddling disk sectors/page cache folios etc.
const _: () = assert!(
    512 % std::mem::size_of::<VoteWalItem>() == 0,
    "WAL items should be sector-aligned",
);

impl ElectionState {
    fn vote(&mut self, req: VoteRequest) -> Result<VoteResponse, (StatusCode, String)> {
        let now = Timestamp::now();
        if now < self.config.voting_period.start {
            return Err((StatusCode::FORBIDDEN, "Voting hasn't started yet".into()));
        }
        if now > self.config.voting_period.end || self.results.is_some() {
            return Err((StatusCode::FORBIDDEN, "Voting is closed".into()));
        }

        if req.candidate as usize >= self.config.candidates.len() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("candidate {} doesn't exist", req.candidate),
            ));
        };
        let voted = &mut self.voted[req.candidate as usize];
        if !voted.contains(&req.psephos.token) {
            let (c, pk) = &self.config.candidates[req.candidate as usize];
            let wal = &mut self.vote_wals[req.candidate as usize];
            if let Err(e) = pk.pk.verify(
                &blind_rsa_signatures::Signature(req.psephos.sig.into()),
                None,
                req.psephos.token,
            ) {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    format!("Bad psephos for {c}: {e:#}"),
                ));
            } else {
                let item = VoteWalItem {
                    psephos: req.psephos.clone(),
                    padding: [0; _],
                };
                wal.write_all(item.as_bytes())
                    .and_then(|_| wal.sync_data())
                    .map_err(|e| {
                        // TODO: seek the WAL back to the end of the last-good item
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Please try again.  {e:#}"),
                        )
                    })?;
            }
            voted.insert(req.psephos.token);
        }
        let receipt = [0; 256]; // TODO self.config.counter_key.sk.sign(blind_msg);
        Ok(VoteResponse { receipt })
    }
}

pub async fn vote_post(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(election): Path<Election>,
    body: Bytes,
) -> Result<Vec<u8>, (StatusCode, String)> {
    let Ok(req) = VoteRequest::from_bytes(&body) else {
        return Err((StatusCode::BAD_REQUEST, "Malformed request".into()));
    };
    let mut state = state.lock().unwrap();
    let state = state.load_election_http(election)?;
    state.vote(req).map(|resp| resp.to_bytes())
}
