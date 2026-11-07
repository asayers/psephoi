use crate::{
    AppState, STATE_DIR, election::ElectionConfig, head, register::RegWalItem, vote::VoteWalItem,
};
use anyhow::{Context, Result, ensure};
use axum::{extract::State, http::StatusCode, response::Redirect};
use axum_extra::extract::Form;
use base64ct::{Base64UrlUnpadded, Encoding};
use jiff::{civil::DateTime, tz::TimeZone};
use maud::{Markup, html};
use psephoi::Election;
use rustix::fs::{FallocateFlags, fallocate};
use std::{
    fs::File,
    sync::{Arc, LazyLock, Mutex},
};
use tracing::error;

const MAX_ELECTIONS: usize = 1000;

pub async fn new_get() -> Markup {
    static RET: LazyLock<Markup> = LazyLock::new(|| {
        html! {
            (maud::DOCTYPE)
            html lang="en" {
                head { (head("New election")) }
                body {
                    header { h1 { "New election" } }
                    main {
                        form method="post" {
                            div {
                            input id="election_name" name="name" type="text"
                            label for="election_name" { "Name" }
                            }
                            div {
                            input id="n_voters" name="n_voters" type="number"
                            label for="n_voters" { "# Voters" }
                            }
                            div {
                            input id="voting_starts" name="voting_starts" type="datetime-local"
                            label for="voting_starts" { "Voting starts" }
                            }
                            div {
                            input id="voting_ends" name="voting_ends" type="datetime-local"
                            label for="voting_ends" { "Voting ends" }
                            }
                            div {
                            input id="timezone" name="timezone" type="text" value="Asia/Tokyo"
                            label for="timezone" { "Timezone" }
                            }

                            p { "Candidates" }
                            ul {
                                li { input name="candidate" type="text"; }
                                li { input name="candidate" type="text"; }
                                li { input name="candidate" type="text"; }
                                li { input name="candidate" type="text"; }
                                li { input name="candidate" type="text"; }
                                li { input name="candidate" type="text"; }
                                li { input name="candidate" type="text"; }
                                li { input name="candidate" type="text"; }
                                li { input name="candidate" type="text"; }
                                li { input name="candidate" type="text"; }
                            }

                            div {
                            input id="allow_reg_while_voting" name="allow_reg_while_voting" type="checkbox" value="true"
                            label for="allow_reg_while_voting" {
                                "Allow registration after voting has begun."
                                b{"Warning:"} " reduces privacy. If a user
                                registers and then immediately votes, it's easy
                                to tell who they voted for." }
                            }

                            button { "Create" }
                        }
                    }
                }
            }
        }
    });
    RET.clone()
}

#[derive(serde::Deserialize)]
pub struct NewRequest {
    name: String,
    n_voters: u32,
    voting_starts: DateTime,
    voting_ends: DateTime,
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
    let voting_starts = req
        .voting_starts
        .to_zoned(tz.clone())
        .map_err(bad_req)?
        .timestamp();
    let voting_ends = req.voting_ends.to_zoned(tz).map_err(bad_req)?.timestamp();
    let voting_period = voting_starts..voting_ends;
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
        ensure!(
            self.elections.len() < MAX_ELECTIONS,
            "election limit reached"
        );
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
