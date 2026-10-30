use crate::{
    AppState, STATE_DIR, election::ElectionConfig, register::RegWalItem, vote::VoteWalItem,
};
use anyhow::{Context, Result};
use axum::{extract::State, http::StatusCode, response::Redirect};
use axum_extra::extract::Form;
use base64ct::{Base64UrlUnpadded, Encoding};
use jiff::{civil::Date, tz::TimeZone};
use psephoi::Election;
use rustix::fs::{FallocateFlags, fallocate};
use std::{
    fs::File,
    sync::{Arc, Mutex},
};
use tracing::error;

#[derive(serde::Deserialize)]
pub struct NewRequest {
    name: String,
    n_voters: u32,
    election_day: Date,
    timezone: String,
    candidate: Vec<String>,
    #[serde(default)]
    allow_reg_while_voting: bool,
}

pub fn bad_req<E: Into<anyhow::Error>>(e: E) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, format!("{:#}", e.into()))
}

pub async fn new_post(
    State(state): State<Arc<Mutex<AppState>>>,
    Form(req): Form<NewRequest>,
) -> Result<Redirect, (StatusCode, String)> {
    let tz = TimeZone::get(&req.timezone).map_err(bad_req)?;
    let day = req.election_day.to_zoned(tz).map_err(bad_req)?;
    let start_of_day = day.timestamp();
    let end_of_day = day.tomorrow().map_err(bad_req)?.timestamp();
    let voting_period = start_of_day..end_of_day;
    let config = ElectionConfig::new(
        req.name,
        req.candidate,
        req.n_voters,
        voting_period,
        req.allow_reg_while_voting,
    )
    .map_err(bad_req)?;
    let mut state = state.lock().unwrap();
    let link = state.create_election(config).map_err(|e| {
        error!("Couldn't create election: {e:#}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "something went wrong".into(),
        )
    })?;
    Ok(Redirect::to(&link))
}

fn create_election(config: ElectionConfig) -> Result<Election> {
    let manifest = config.manifest();
    let hash = manifest.hash();
    let dir = STATE_DIR.join(hash.to_string());

    std::fs::create_dir(&dir).context("create dir")?;
    std::fs::write(
        dir.join("config.ron"),
        ron::ser::to_string_pretty(&config, ron::ser::PrettyConfig::new())?,
    )?;
    std::fs::write(dir.join("manifest.bin"), manifest.to_bytes())?;
    preallocate_file::<RegWalItem>(&dir.join("registrations.wal"), config.n_voters)?;
    for i in 0..config.candidates.len() {
        preallocate_file::<VoteWalItem>(&dir.join(format!("votes_{i}.wal")), config.n_voters)?;
    }
    // TODO: Preallocate the results files (nonces_i.bin, sigs_i.bin, results.bin, registrations.bin)
    Ok(hash)
}

impl AppState {
    fn create_election(&mut self, config: ElectionConfig) -> Result<String> {
        let hash = create_election(config)?;
        let estate = self
            .load_election(hash)?
            .context("Just-written election is missing")?;

        let key = blake3::keyed_hash(&estate.config.registrar_key, hash.0.as_bytes());
        let key = Base64UrlUnpadded::encode_string(key.as_bytes());
        Ok(format!("/{hash}/organiser?key={key}"))
    }
}

/// This ensures we don't run out of space half-way through
fn preallocate_file<Item>(path: &std::path::Path, max_items: u32) -> Result<()> {
    let f = File::create_new(path)?;
    let size = std::mem::size_of::<Item>() * max_items as usize;
    fallocate(&f, FallocateFlags::KEEP_SIZE, 0, size as u64)?;
    Ok(())
}
