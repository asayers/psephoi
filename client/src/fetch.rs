use crate::JsResultExt;
use anyhow::{Context, Result, bail, ensure};
use async_once_cell::OnceCell;
use base64ct::{Base64UrlUnpadded, Encoding};
use blake3::Hash;
use gloo_net::http::Request;
use gloo_utils::window;
use psephoi::{Election, ElectionManifest, ElectionResults};
use tracing::info;

pub async fn election() -> Result<Election> {
    static ELECTION: OnceCell<Election> = OnceCell::new();
    ELECTION
        .get_or_try_init(async {
            let txt = window().location().pathname().js()?;
            let bytes = Base64UrlUnpadded::decode_vec(txt.trim_start_matches('/'))
                .context("election ID should be base64url-encoded")?;
            let bytes = bytes
                .try_into()
                .ok()
                .context("election ID should be 43 characters long")?;
            let hash = Hash::from_bytes(bytes);
            Ok(Election(hash))
        })
        .await
        .copied()
}

pub async fn get_manifest() -> Result<&'static ElectionManifest> {
    static MANIFEST: OnceCell<ElectionManifest> = OnceCell::new();
    MANIFEST
        .get_or_try_init(async {
            let election = election().await?;
            let path = format!("/{election}/manifest");
            info!("Requesting manifest from {path}");
            let resp = Request::get(&path).send().await.unwrap();
            if !resp.ok() {
                let err = resp.text().await.unwrap();
                bail!("{err}");
            }
            let manifest = resp.binary().await.unwrap();
            let actual = blake3::hash(&manifest);
            ensure!(actual == election.0, "Manifest is corrupt");
            Ok(ElectionManifest::from_bytes(&manifest).unwrap())
        })
        .await
}

pub async fn get_results() -> Result<&'static ElectionResults> {
    static RESULTS: OnceCell<ElectionResults> = OnceCell::new();
    RESULTS
        .get_or_try_init(async {
            let election = election().await?;
            let path = format!("/{election}/results");
            info!("Requesting results from {path}");
            let resp = Request::get(&path).send().await?;
            if !resp.ok() {
                let err = resp.text().await?;
                bail!("{err}");
            }
            let resp = resp.binary().await?;
            let results = ElectionResults::from_bytes(&resp)?;
            assert_eq!(results.election, *election.0.as_bytes());
            Ok(results)
        })
        .await
}
