use crate::{Candidate, register::RegWalItem, vote::VoteWalItem};
use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use linearize::{LinearizeExt, StaticCopyMap};
use psephoi::{CandidateManifest, ElectionManifest, KeyPair, NONCE_LEN, Phase, SIG_LEN};
use rand::Rng;
use std::{ops::Range, time::Duration};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ElectionConfig {
    pub name: String,
    pub candidates: Vec<(Candidate, KeyPair)>,
    pub n_voters: u32,
    pub registrar_key: [u8; 32],
    pub counter_key: KeyPair,
    pub voting_period: Range<Timestamp>,
    pub allow_reg_while_voting: bool,
}

const MAX_VOTERS: u32 = 200;
const MAX_CANDIDATES: usize = 10;
const MAX_NAME_LEN: usize = 100;
const MAX_BYTES: usize = 1024 * 1024; // 1 MiB, enough for 200 voters and 10 candidates
const MAX_VOTING_DURATION: Duration = Duration::from_secs(7 * 24 * 60 * 60); // ~1 week
const MAX_REGISTERING_DURATION: Duration = Duration::from_secs(31 * 24 * 60 * 60); // ~1 month
const RETENTION_DURATION: Duration = Duration::from_secs(31 * 24 * 60 * 60);

impl ElectionConfig {
    pub fn new(
        name: String,
        mut candidates: Vec<String>,
        n_voters: u32,
        voting_period: Range<Timestamp>,
        allow_reg_while_voting: bool,
    ) -> Result<Self> {
        let durations = phase_durations(voting_period.clone(), allow_reg_while_voting)?;

        ensure!(name.len() <= MAX_NAME_LEN, "Election name too long");

        ensure!(n_voters > 0, "No voters");
        ensure!(n_voters <= MAX_VOTERS, "Too many voters");

        candidates.retain_mut(|x| {
            *x = x.trim().to_string();
            !x.is_empty()
        });
        candidates.sort();
        candidates.dedup();
        ensure!(!candidates.is_empty(), "No candidates");
        ensure!(candidates.len() <= MAX_CANDIDATES, "Too many candidates");
        ensure!(
            candidates.iter().all(|x| x.len() <= MAX_NAME_LEN),
            "Candidate name too long"
        );

        let costs = disk_usage(n_voters as usize, candidates.len());
        for p in Phase::variants() {
            println!(
                "Cost during {p:?}: {}B (max) - {:?}",
                costs[p], durations[p]
            );
        }
        ensure!(costs.values().copied().max().unwrap() < MAX_BYTES);

        let mut registrar_key = [0; 32];
        rand::rng().fill_bytes(&mut registrar_key[..]);
        Ok(ElectionConfig {
            name,
            candidates: candidates
                .into_iter()
                .map(|name| Ok((Candidate(name), KeyPair::generate()?)))
                .collect::<Result<_>>()?,
            n_voters,
            registrar_key,
            counter_key: KeyPair::generate()?,
            voting_period,
            allow_reg_while_voting,
        })
    }

    pub fn manifest(&self) -> ElectionManifest {
        ElectionManifest {
            name: self.name.as_bytes().into(),
            candidates: self
                .candidates
                .iter()
                .map(|(name, keypair)| CandidateManifest {
                    name: name.0.as_bytes().into(),
                    pubkey: keypair.pk.to_spki().unwrap().into(),
                })
                .collect(),
            n_voters: self.n_voters,
            voting_start: self.voting_period.start.as_second() as u64,
            voting_end: self.voting_period.end.as_second() as u64,
            reg_while_voting: if self.allow_reg_while_voting {
                psephoi::RegWhileVoting::Allow
            } else {
                psephoi::RegWhileVoting::Disallow
            },
        }
    }

    pub fn registration_closes(&self) -> Timestamp {
        if self.allow_reg_while_voting {
            self.voting_period.end
        } else {
            self.voting_period.start
        }
    }
}

pub fn phase_durations(
    voting_period: Range<Timestamp>,
    allow_reg_while_voting: bool,
) -> Result<StaticCopyMap<Phase, Duration>> {
    let registering_duration = if allow_reg_while_voting {
        voting_period.end
    } else {
        voting_period.start
    }
    .duration_since(Timestamp::now())
    .try_into()
    .context("No registration period")?;
    let voting_duration = voting_period
        .end
        .duration_since(voting_period.start)
        .try_into()
        .context("No voting period")?;
    ensure!(
        registering_duration <= MAX_REGISTERING_DURATION,
        "Registering period too long"
    );
    ensure!(
        voting_duration <= MAX_VOTING_DURATION,
        "Voting period too long"
    );
    Ok(StaticCopyMap::from_fn(|p| match p {
        Phase::Registration => registering_duration,
        Phase::Voting => voting_duration,
        Phase::Finished => RETENTION_DURATION,
    }))
}

/// A strict _upper bound_ on the amount of disk required.  Actual usage may be much less
fn disk_usage(n_voters: usize, n_candidates: usize) -> StaticCopyMap<Phase, usize /* bytes */> {
    StaticCopyMap::from_fn(|phase| match phase {
        Phase::Registration => std::mem::size_of::<RegWalItem>() * n_voters,
        Phase::Voting => std::mem::size_of::<VoteWalItem>() * n_voters * n_candidates,
        Phase::Finished => {
            let reg_bitmap = n_voters / 8;
            let n = n_voters * n_candidates;
            let nonces = NONCE_LEN * n;
            let sigs = SIG_LEN * n;
            // let tree = nonces / 64; // or something
            reg_bitmap + nonces + sigs
        }
    })
}
