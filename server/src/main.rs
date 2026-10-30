mod assets;
mod election;
mod new;
mod organiser;
mod register;
mod results;
mod vote;

use crate::assets::{Asset, asset_url, static_asset};
use crate::election::ElectionConfig;
use crate::new::new_post;
use crate::organiser::organiser_get;
use crate::register::{RegHash, read_registrations, register_post, registrations_get};
use crate::results::{manifest_get, nonces_get, results_get, sigs_get};
use crate::vote::{read_votes, vote_post};
use anyhow::{Context, Result};
use axum::extract::{DefaultBodyLimit, MatchedPath};
use axum::http::{HeaderValue, header};
use axum::{
    Router,
    extract::Path,
    http::StatusCode,
    routing::{get, post},
};
use maud::{Markup, PreEscaped, html};
use psephoi::*;
use roaring::RoaringBitmap;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Default)]
pub struct AppState {
    elections: HashMap<Election, ElectionState>,
}

#[derive(Debug)]
pub struct ElectionState {
    pub dir: PathBuf,
    pub config: ElectionConfig,
    pub registered: Vec<Option<RegHash>>,        // len=n_voters
    pub registered_final: Option<RoaringBitmap>, // A cache of registrations.bin
    pub voted: Vec<HashSet<[u8; NONCE_LEN]>>,    // one per candidate
    pub hash: Election,                          // cached manifest hash
    pub reg_wal: File,
    pub vote_wals: Vec<File>,             // one per candidate
    pub results: Option<ElectionResults>, // A cache of results.bin
}

impl ElectionState {
    pub fn load(election: Election) -> Result<Option<Self>> {
        let dir = STATE_DIR.join(election.to_string());
        let config = match std::fs::read_to_string(dir.join("config.ron")) {
            Ok(x) => x,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => Err(e).context("opening config.ron")?,
        };
        let config: ElectionConfig = ron::from_str(&config)?;
        assert_eq!(config.manifest().hash(), election);

        // Replay (and repair) the WALs
        let registered = read_registrations(&dir, &config)?;
        let voted = read_votes(&dir, &config)?
            .into_iter()
            .map(|x| x.keys().copied().collect())
            .collect();
        let registered_final = match File::open(dir.join("registrations.bin")) {
            Ok(f) => Some(RoaringBitmap::deserialize_from(f)?),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => Err(e)?,
        };
        let results = match std::fs::read(dir.join("results.bin")) {
            Ok(bytes) => Some(ElectionResults::from_bytes(&bytes)?),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => Err(e)?,
        };

        Ok(Some(ElectionState {
            hash: election,
            reg_wal: File::options()
                .append(true)
                .open(dir.join("registrations.wal"))?,
            vote_wals: (0..config.candidates.len())
                .map(|i| {
                    anyhow::Ok(
                        File::options()
                            .append(true)
                            .open(dir.join(format!("votes_{i}.wal")))?,
                    )
                })
                .collect::<Result<_>>()?,
            registered,
            registered_final,
            voted,
            config,
            dir,
            results,
        }))
    }
}

impl AppState {
    fn load_election_http(
        &mut self,
        name: Election,
    ) -> Result<&mut ElectionState, (StatusCode, String)> {
        match self.load_election(name) {
            Ok(Some(x)) => Ok(x),
            Ok(None) => Err((StatusCode::NOT_FOUND, format!("{name} not found"))),
            Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, format!("{name}: {e:#}"))),
        }
    }

    fn load_election(&mut self, name: Election) -> Result<Option<&mut ElectionState>> {
        use std::collections::hash_map::Entry;
        match self.elections.entry(name) {
            Entry::Occupied(entry) => Ok(Some(entry.into_mut())),
            Entry::Vacant(entry) => match ElectionState::load(name)? {
                Some(x) => Ok(Some(entry.insert(x))),
                None => Ok(None),
            },
        }
    }
}

static STATE_DIR: LazyLock<PathBuf> = LazyLock::new(|| {
    PathBuf::from(std::env::var("STATE_DIRECTORY").unwrap_or_else(|_| "./state".into()))
});

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .without_time()
        .init();

    let state = Arc::new(Mutex::new(AppState::default()));

    let app = Router::new()
        .route("/", get(root))
        .route("/new", get(new_get).post(new_post))
        .route("/assets/{hash}", get(static_asset))
        .route("/{election}/manifest", get(manifest_get))
        .route("/{election}/organiser", get(organiser_get))
        .route("/{election}/register", post(register_post))
        .route("/{election}/vote", post(vote_post))
        .route("/{election}/registrations", get(registrations_get))
        .route("/{election}/results", get(results_get))
        .route("/{election}/nonces/{i}", get(nonces_get))
        .route("/{election}/sigs/{i}", get(sigs_get))
        .route("/{election}", get(election_get))
        .with_state(state)
        .layer(DefaultBodyLimit::max(8 * 1024))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(10),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &axum::extract::Request| {
                let path = req.extensions().get::<MatchedPath>().map(|p| p.as_str());
                tracing::info_span!("req", method = %req.method(), path)
            }),
        );

    let mut fds = listenfd::ListenFd::from_env();
    match fds.take_unix_listener(0)? {
        Some(sock) => {
            info!("Got socket via fd-passing");
            sock.set_nonblocking(true)?;
            let sock = tokio::net::UnixListener::from_std(sock)?;
            Ok(axum::serve(sock, app).await?)
        }
        None => {
            let addr = "127.0.0.1:7537";
            info!("Listening on {addr}");
            let sock = TcpListener::bind(addr).await?;
            Ok(axum::serve(sock, app).await?)
        }
    }
}

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
                            input id="election_day" name="election_day" type="date"
                            label for="election_day" { "Election day" }
                            }
                            div {
                            input id="timezone" name="timezone" type="text"
                            label for="timezone" { "Timezone" }
                            }
                            div {
                            input id="allow_reg_while_voting" name="allow_reg_while_voting" type="checkbox" value="true"
                            label for="allow_reg_while_voting" { "Allow registration after voting has begun" }
                            p { b{"Warning:"} " reduces privacy. If a user registers
                            and then immediately votes, it's easy to tell who
                            they voted for." }
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

                            button { "Create" }
                        }
                    }
                }
            }
        }
    });
    RET.clone()
}

pub fn head(title: &str) -> Markup {
    html! {
        meta charset="utf-8";
        meta name="viewport" content="width=device-width, initial-scale=1";
        title { (title) };
        link rel="stylesheet" href=(asset_url(Asset::Css));
    }
}

pub async fn election_get(Path(election): Path<Election>) -> Result<Markup, (StatusCode, String)> {
    let exists = std::fs::exists(STATE_DIR.join(election.to_string())).is_ok_and(|x| x);
    if !exists {
        return Err((StatusCode::NOT_FOUND, format!("{election} not found")));
    }
    let importmap = format!(
        r#"{{"imports":{{"client":"{}","client.wasm":"{}"}}}}"#,
        asset_url(Asset::Bindings),
        asset_url(Asset::Wasm)
    );
    Ok(html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                (head("Loading..."))
                script type="importmap" { (PreEscaped(importmap)) }
                link rel="modulepreload" href=(asset_url(Asset::Bindings));
                link rel="preload" href=(asset_url(Asset::Wasm)) as="fetch" crossorigin;
                script type="module" src=(asset_url(Asset::Glue)) {}
            }
            body { "Loading..." }
        }
    })
}

pub async fn root() -> Markup {
    html! {
        (maud::DOCTYPE)
        html lang="en" {
            head { (head("Psephoi")) }
            body {
                header { h1 { "Psephoi" } }
                main {
                    p { "An implementation of approval voting with the following properties:" }
                    ul {
                        li { em { "Verifiability:"} " You can verify that your vote was included in the tally" }
                        li { em { "Secrecy:"} " No-one can tell how you voted" }
                    }
                    form method="get" action="/new" { button { "Create election" } }
                }
            }
        }
    }
}
