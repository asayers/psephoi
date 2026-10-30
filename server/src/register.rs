use crate::{
    AppState, Election, ElectionState,
    election::ElectionConfig,
    new::bad_req,
    results::{atomic_write, static_get},
};
use anyhow::{Context, Result, anyhow, ensure};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use jiff::Timestamp;
use psephoi::{RegRequest, RegResponse, RegToken};
use roaring::RoaringBitmap;
use rustix::fs::{FallocateFlags, fallocate, ftruncate};
use std::{
    fs::File,
    io::{ErrorKind, Write},
    num::NonZero,
    sync::{Arc, Mutex},
};
use tracing::warn;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, LittleEndian, U32};

pub struct VoterId(pub NonZero<u32>);

/// Hash of a `RegRequest`
#[derive(Copy, Clone, PartialEq, Eq, Debug, KnownLayout, FromBytes, IntoBytes, Immutable)]
#[repr(transparent)]
pub struct RegHash([u8; 28]); // Reduced, but still plenty

#[derive(KnownLayout, FromBytes, IntoBytes, Immutable)]
#[repr(C)]
pub struct RegWalItem {
    voter_id: U32<LittleEndian>,
    req_hash: RegHash,
    // TODO: checksum?
}

// Avoid straddling disk sectors/page cache folios etc.
const _: () = assert!(
    512 % std::mem::size_of::<RegWalItem>() == 0,
    "WAL items should be sector-aligned",
);

pub fn read_registrations(
    dir: &std::path::Path,
    config: &ElectionConfig,
) -> Result<Vec<Option<RegHash>>> {
    let mut registered = vec![None; config.n_voters as usize];
    let path = dir.join("registrations.wal");
    let wal_mmap = match mmap_snapshot::Mmap::open(&path) {
        Ok(x) => x,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => Err(e)?,
    };
    let (wal, suffix) =
        <[RegWalItem]>::ref_from_prefix(&wal_mmap).map_err(|e| anyhow!("Corrupt WAL: {e}"))?;
    for item in wal {
        ensure!(
            (1..=config.n_voters).contains(&item.voter_id.get()),
            "Corrupt WAL"
        );
        let i = item.voter_id.get() as usize - 1;
        assert!(registered[i].is_none());
        registered[i] = Some(item.req_hash);
    }
    let new_len = (!suffix.is_empty()).then_some(wal_mmap.len() - suffix.len());
    std::mem::drop(wal_mmap);
    let f = File::options().write(true).open(&path)?;
    if let Some(new_len) = new_len {
        warn!("Discarding partial write at end of WAL");
        ftruncate(&f, new_len as u64)?;
    }
    let size = std::mem::size_of::<RegWalItem>() * config.n_voters as usize;
    fallocate(&f, FallocateFlags::KEEP_SIZE, 0, size as u64)?;
    Ok(registered)
}

impl ElectionState {
    pub fn generate_token(&self, voter_id: VoterId) -> RegToken {
        assert!(voter_id.0.get() <= self.config.n_voters);
        let hash = blake3::keyed_hash(&self.config.registrar_key, &voter_id.0.get().to_be_bytes());
        RegToken {
            voter_id: voter_id.0.get(),
            mac: hash.as_bytes()[..16].try_into().unwrap(),
        }
    }

    pub fn register(&mut self, req: RegRequest) -> Result<RegResponse, (StatusCode, String)> {
        if Timestamp::now() > self.config.registration_closes() || self.registered_final.is_some() {
            return Err((StatusCode::FORBIDDEN, "Registration is closed".into()));
        }
        let hash = blake3::keyed_hash(
            &self.config.registrar_key,
            &req.token.voter_id.to_be_bytes(),
        );
        if !constant_time_eq::constant_time_eq_16(
            &hash.as_bytes()[..16].try_into().unwrap(),
            &req.token.mac,
        ) {
            return Err((StatusCode::UNAUTHORIZED, "voter token is malformed".into()));
        }
        let voter_id = VoterId(NonZero::new(req.token.voter_id).unwrap());
        assert!(voter_id.0.get() <= self.config.n_voters);
        if req.blinded_numbers.len() != self.config.candidates.len() {
            return Err((StatusCode::BAD_REQUEST, "wrong number of candidates".into()));
        }
        let blinded_sigs = self
            .config
            .candidates
            .iter()
            .zip(req.blinded_numbers.iter())
            .map(|((_, keypair), blinded_number)| {
                Ok(keypair
                    .sk
                    .blind_sign(blinded_number.as_slice())?
                    .0
                    .try_into()
                    .unwrap())
            })
            .collect::<Result<_>>()
            .context("Tokens were incorrectly blinded")
            .map_err(bad_req)?;
        // Malformed input should all be rejected by this point.  Time to commit the registration
        let req_hash = RegHash(
            blake3::keyed_hash(&self.config.registrar_key, &req.to_bytes()).as_bytes()[..28]
                .try_into()
                .unwrap(),
        );
        let i = voter_id.0.get() as usize - 1;
        if let Some(hash) = self.registered[i] {
            if hash == req_hash {
                // this is a retry - carry on
            } else {
                return Err((
                    StatusCode::FORBIDDEN,
                    format!("you've already registered for {}", self.hash),
                ));
            }
        } else {
            self.reg_wal
                .write_all(
                    RegWalItem {
                        voter_id: U32::new(voter_id.0.get()),
                        req_hash,
                    }
                    .as_bytes(),
                )
                .and_then(|_| self.reg_wal.sync_data())
                .map_err(|e| {
                    // TODO: seek the WAL back to the end of the last-good item
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Please try again.  {e:#}"),
                    )
                })?;
            self.registered[i] = Some(req_hash);
        }
        Ok(RegResponse { blinded_sigs })
    }
}

// TODO: Allow the client to retry for a few minutes: identical request => repeat the response.
pub async fn register_post(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(election): Path<Election>,
    body: Bytes,
) -> Result<Vec<u8>, (StatusCode, String)> {
    let Ok(req) = RegRequest::from_bytes(&body) else {
        return Err((StatusCode::BAD_REQUEST, "Malformed request".into()));
    };
    let mut state = state.lock().unwrap();
    let state = state.load_election_http(election)?;

    match state.register(req) {
        Ok(resp) => Ok(resp.to_bytes()),
        Err((code, error)) => Err((code, error)),
    }
}

pub async fn registrations_get(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(election): Path<Election>,
) -> Result<Response, (StatusCode, String)> {
    let mut state = state.lock().unwrap();
    let state = state.load_election_http(election)?;
    let fname = "registrations.bin";
    let now = Timestamp::now();
    if now > state.config.registration_closes() && state.registered_final.is_none() {
        let mut bitmap: RoaringBitmap = state
            .registered
            .iter()
            .enumerate()
            .filter(|(_, x)| x.is_some())
            .map(|(i, _)| i as u32 + 1)
            .collect();
        bitmap.optimize();
        // TODO: Sign it
        let mut bytes = vec![];
        bitmap.serialize_into(&mut bytes).unwrap();
        atomic_write(&state.dir, fname, &bytes)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
        state.registered_final = Some(bitmap);
    }
    static_get(election, fname)
}
