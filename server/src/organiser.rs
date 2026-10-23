use crate::{
    AppState,
    assets::{Asset, asset_url},
    register::VoterId,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
};
use base64ct::{Base64UrlUnpadded, Encoding};
use constant_time_eq::constant_time_eq;
use maud::{Markup, html};
use psephoi::{Election, render_manifest};
use std::{
    num::NonZero,
    sync::{Arc, Mutex},
};

#[derive(serde::Deserialize)]
pub struct OrganiserQuery {
    key: String,
}

pub async fn organiser_get(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(election): Path<Election>,
    Query(query): Query<OrganiserQuery>,
) -> Result<Markup, (StatusCode, String)> {
    let mut state = state.lock().unwrap();
    let state = state.load_election_http(election)?;
    let expected = blake3::keyed_hash(&state.config.registrar_key, election.0.as_bytes());
    let actual = Base64UrlUnpadded::decode_vec(&query.key)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "bad key".into()))?;
    if !constant_time_eq(&actual, expected.as_bytes()) {
        return Err((StatusCode::UNAUTHORIZED, "bad key".into()));
    }
    let manifest = state.config.manifest();
    let links = (0..state.config.n_voters).map(|i| {
        let token = state.generate_token(VoterId(NonZero::new(i + 1).unwrap()));
        let magic_link = format!(
            "https://vote.asayers.com/{}#{}",
            election,
            Base64UrlUnpadded::encode_string(&token.to_bytes())
        );
        let claimed = state.registered[token.voter_id as usize - 1].is_some();
        (magic_link, claimed)
    });
    let results = manifest
        .candidates
        .iter()
        .zip(&state.voted)
        .map(|(m, ns)| (m.name(), ns.len()));
    Ok(html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Psephoi" };
                link rel="stylesheet" href=(asset_url(Asset::Css));
            }
            body {
                header { h1 { (&state.config.name) } }
                main {
                    (render_manifest(&manifest))
                    h2 { "Registration magic links" }
                    ol {
                        @for (link, claimed) in links {
                            li { @if claimed { del { (link) } } @else { a href=(link) { (link) } } }
                        }
                    }
                    h2 { "Results" }
                    ul {
                        @for (c, n) in results {
                            li { (c) ": " (n) }
                        }
                    }
                }
            }
        }
    })
}
