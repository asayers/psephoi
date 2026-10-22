use crate::{AppState, Candidate, Election};
use aegis::aegis128l::Aegis128L;
use anyhow::Result;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rand::{Rng, rng};
use roaring::RoaringBitmap;
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const KEYLEN: usize = 2048; // bits

/// RSABSSA-SHA384-PSS-Deterministic
type KeyPair = blind_rsa_signatures::KeyPair<
    blind_rsa_signatures::Sha384,
    blind_rsa_signatures::PSS,
    blind_rsa_signatures::Deterministic,
>;

#[derive(Default)]
pub struct RegistrarState {
    pub elections: HashMap<Election, ElectionState>,
}

#[derive(Debug)]
pub struct ElectionState {
    pub name: String,
    pub candidates: HashMap<Candidate, CandidateState>,
    pub unregistered: RoaringBitmap,
    pub key: [u8; 16],
}

impl ElectionState {
    pub fn new(name: String) -> Self {
        let mut key = [0; 16];
        rand::rng().fill_bytes(&mut key[..]);
        let n_electors = 10u32;
        let mut unregistered = RoaringBitmap::new();
        for i in 0..n_electors {
            let mut nonce = [0; 16];
            rand::rng().fill_bytes(&mut nonce[..]);
            let mut buf = i.to_be_bytes();
            let tag = Aegis128L::<16>::new(&key, &nonce).encrypt_in_place(&mut buf, b"");
            let params = RegToken {
                nonce,
                tag,
                body: buf,
            };
            println!("{params:?}");
            unregistered.insert(i);
        }
        ElectionState {
            name,
            candidates: HashMap::default(),
            unregistered,
            key,
        }
    }
}

#[derive(Debug)]
pub struct CandidateState {
    pub keypair: KeyPair,
}

impl CandidateState {
    pub fn new() -> Result<Self> {
        Ok(CandidateState {
            keypair: KeyPair::generate(&mut rng(), KEYLEN)?,
        })
    }
}

/// A bearer token that proves eligibility to vote.  Used up after registration
// TODO: tls-codec and base64-encode, or something
#[derive(Deserialize, Debug)]
pub struct RegToken {
    nonce: [u8; 16],
    tag: [u8; 16],
    body: [u8; 4],
}
#[derive(Deserialize)]
pub struct RegParams {
    token: RegToken,
    blinded_numbers: Vec<[u8; 16]>,
}
pub async fn registrar_post(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(election): Path<Election>,
    Json(params): Json<RegParams>,
) -> (StatusCode, String) {
    let mut state = state.lock().unwrap();
    let Some(state) = state.registrar.elections.get_mut(&election) else {
        return (StatusCode::NOT_FOUND, format!("{election} not found"));
    };
    let mut buf = params.token.body;
    if Aegis128L::<16>::new(&state.key, &params.token.nonce)
        .decrypt_in_place(&mut buf, &params.token.tag, b"")
        .is_err()
    {
        return (
            StatusCode::UNAUTHORIZED,
            format!("you're not eligible to vote in {election}"),
        );
    }
    let i = u32::from_be_bytes(buf);
    if params.blinded_numbers.len() != state.candidates.len() {
        return (StatusCode::BAD_REQUEST, "wrong number of candidates".into());
    }
    if !state.unregistered.remove(i) {
        return (
            StatusCode::FORBIDDEN,
            format!("you've already registered for {election}"),
        );
    }
    let mut ret = HashMap::new();
    for ((candidate, state), blinded_number) in state.candidates.iter().zip(params.blinded_numbers)
    {
        let sig = state.keypair.sk.blind_sign(blinded_number).unwrap();
        ret.insert(candidate, sig);
    }
    (
        StatusCode::OK,
        format!("registered for {election}\n{ret:?}"),
    )
}
