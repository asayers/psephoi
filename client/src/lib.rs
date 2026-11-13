mod buttons;
mod fetch;
mod pre_psephoi;
mod render;

use crate::buttons::*;
use crate::fetch::*;
use crate::pre_psephoi::*;
use crate::render::*;
use anyhow::Context;
use anyhow::{Result, anyhow};
use base64ct::{Base64UrlUnpadded, Encoding};
use gloo_utils::document;
use psephoi::Psephoi;
use tls_codec::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::Element;

#[wasm_bindgen(start)]
pub async fn init() -> Result<(), JsError> {
    init_().await.map_err(|e| JsError::new(&format!("{e:#}")))
}

pub async fn init_() -> Result<()> {
    console_error_panic_hook::set_once();

    let manifest = get_manifest().await?;
    document().set_title(manifest.name());
    ensure_pre_psephoi_exist().await?;
    let markup = render().await?;
    body()?.set_inner_html(&markup.into_string());
    wire_up_buttons().await;
    Ok(())
}

fn body() -> Result<Element> {
    Ok(document().query_selector("body").js()?.unwrap())
}

/// All this voter's psephoi, sent and unsent
async fn retrieve_psephoi() -> Result<Option<Psephoi>> {
    let election = election().await?;
    let storage_key = format!("psephoi_{}", election);
    let val = local_storage()?.get_item(&storage_key).js()?;
    val.map(|encoded| {
        let bytes = Base64UrlUnpadded::decode_vec(&encoded)?;
        Ok(Psephoi::tls_deserialize_exact(&bytes)?)
    })
    .transpose()
}

/// The union of all votes to date (for this election, by this voter)
async fn released_psephoi() -> Result<Vec<&'static str>> {
    let election = election().await?;
    let manifest = get_manifest().await?;
    let mut cs = vec![];
    for (i, c) in manifest.candidates.iter().enumerate() {
        let storage_key = format!("receipt_{}_{}", election, i);
        if local_storage()?.get_item(&storage_key).js()?.is_some() {
            cs.push(c.name());
        }
    }
    Ok(cs)
}

trait JsResultExt<T> {
    fn js(self) -> anyhow::Result<T>;
}
impl<T> JsResultExt<T> for Result<T, JsValue> {
    fn js(self) -> anyhow::Result<T> {
        self.map_err(|v| match v.dyn_into::<js_sys::Error>() {
            Ok(e) => anyhow!("{}: {}", String::from(e.name()), String::from(e.message())),
            Err(v) => anyhow!("{v:?}"),
        })
    }
}

fn local_storage() -> Result<web_sys::Storage> {
    gloo_utils::window()
        .local_storage()
        .js()?
        .context("localStorage unavailable")
}
