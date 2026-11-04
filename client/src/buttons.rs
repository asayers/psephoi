use crate::JsResultExt;
use crate::body;
use crate::fetch::election;
use crate::fetch::get_manifest;
use crate::fetch::get_results;
use crate::pre_psephoi::get_pre_psephoi;
use crate::render::render;
use crate::render::status_box;
use crate::retrieve_psephoi;
use anyhow::{Context, Result, anyhow, bail, ensure};
use base64ct::{Base64UrlUnpadded, Encoding};
use gloo_events::EventListener;
use gloo_net::http::Request;
use gloo_storage::Storage;
use gloo_utils::document;
use gloo_utils::window;
use linearize::Linearize;
use linearize::LinearizeExt;
use psephoi::{
    NONCE_LEN, Psephoi, Psephos, PublicKey, RegRequest, RegResponse, RegToken, VoteRequest,
    VoteResponse,
};
use tracing::info;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use zerocopy::FromBytes;

#[derive(Copy, Clone, Debug, Linearize)]
enum Button {
    Register,
    Vote,
    Verify,
    Audit,
}

pub async fn wire_up_buttons() {
    for button in Button::variants() {
        let el = document()
            .get_element_by_id(&format!("btn-{button:?}"))
            .unwrap();
        EventListener::new(&el.clone(), "click", move |_| {
            el.set_attribute("disabled", "").unwrap();
            let status = match button {
                Button::Register => "Registering...",
                Button::Vote => "Submitting vote...",
                Button::Verify => "Locating your vote...",
                Button::Audit => "Auditing the results...",
            };
            status_box().set_text_content(Some(status));
            let el = el.clone();
            spawn_local(async move {
                let res = match button {
                    Button::Register => register().await,
                    Button::Vote => vote().await,
                    Button::Verify => verify().await,
                    Button::Audit => audit().await,
                };
                match res {
                    Ok(_) => {
                        let markup = render().await.unwrap();
                        body().unwrap().set_inner_html(&markup.into_string());
                    }
                    Err(e) => {
                        status_box().set_text_content(Some(&format!("{e:#}")));
                        // Assume the error is transient, let the user retry
                        el.remove_attribute("disabled").unwrap();
                    }
                }
            });
        })
        .forget();
    }
}

async fn register() -> Result<()> {
    let election = election().await?;

    ensure!(
        retrieve_psephoi().await?.is_none(),
        "You've already got psephoi for this election"
    );

    let pre_psephoi = get_pre_psephoi().await?;

    let manifest = get_manifest().await?;

    let token = window().location().hash().js()?;
    let token = RegToken::from_bytes(&Base64UrlUnpadded::decode_vec(
        token.trim_start_matches('#'),
    )?)?;
    ensure!(token.voter_id > 0 && token.voter_id <= manifest.n_voters);

    let blinded_numbers = pre_psephoi
        .iter()
        .map(|x| x.blind_message.clone())
        .collect();
    let req = RegRequest {
        token,
        blinded_numbers,
    };
    let path = format!("/{election}/register");
    info!("Sending blinded tokens to registrar at {path}");
    let resp = Request::post(&path).body(req.to_bytes())?.send().await?;
    if !resp.ok() {
        let err = resp.text().await?;
        bail!("{err}");
    }
    let resp = resp.binary().await?;
    let resp = RegResponse::from_bytes(&resp)?;
    assert!(resp.blinded_sigs.len() == req.blinded_numbers.len());
    info!("Got response");
    let psephoi = Psephoi(
        manifest
            .candidates
            .iter()
            .zip(pre_psephoi.iter())
            .zip(resp.blinded_sigs.iter())
            .map(|((c, pre_phephos), sig)| {
                info!("Unblinding token for {}...", c.name());
                let pk = PublicKey::from_spki(c.pubkey.as_slice())?;
                let sig = blind_rsa_signatures::BlindSignature(sig.into());
                let res = pre_phephos.clone().into();
                let token = pre_phephos.unblind_message;
                let sig = pk.finalize(&sig, &res, token)?;
                let psephos = Psephos {
                    token,
                    sig: sig
                        .0
                        .try_into()
                        .map_err(|_| anyhow!("sig was wrong length"))?,
                };
                info!("Got a psephos");
                anyhow::Ok(psephos)
            })
            .collect::<Result<_>>()?,
    );
    let encoded = Base64UrlUnpadded::encode_string(&psephoi.to_bytes());
    info!(
        "Successfully registered!  Here are your psephoi:\n{}",
        encoded
    );
    let storage_key = format!("psephoi_{}", election);
    gloo_storage::LocalStorage::set(storage_key, encoded).unwrap();
    Ok(())
}

async fn vote() -> Result<()> {
    let election = election().await?;
    let psephoi = retrieve_psephoi().await?.context("Not registered")?;
    for (i, p) in psephoi.0.iter().enumerate() {
        let checkbox = document()
            .get_element_by_id(&format!("candidate-{i}"))
            .unwrap();
        let checkbox = checkbox
            .dyn_into::<HtmlInputElement>()
            .map_err(|_| anyhow!("candidate-{i} isn't an <input>"))?;
        if checkbox.checked() {
            let storage_key = format!("receipt_{}_{}", election, i);
            let req = VoteRequest {
                candidate: i as u32,
                psephos: p.clone(),
            };
            // Record the intent to send, replace it with the actual receipt when we get it
            gloo_storage::LocalStorage::set(&storage_key, "leaked").unwrap();
            let path = format!("/{election}/vote");
            info!("Sending chosen psephoi to ballot counter at {path}");
            let resp = Request::post(&path).body(req.to_bytes())?.send().await?;
            if !resp.ok() {
                let err = resp.text().await?;
                bail!("{err}");
            }
            let resp = resp.binary().await?;
            let resp = VoteResponse::from_bytes(&resp)?;
            // TODO: Verify that the recipt is properly signed
            let encoded = Base64UrlUnpadded::encode_string(&resp.receipt);
            gloo_storage::LocalStorage::set(storage_key, encoded).unwrap();
        }
    }
    Ok(())
}

async fn verify() -> Result<()> {
    let election = election().await?;
    let psephoi = retrieve_psephoi().await?.context("Not registered")?;
    let manifest = get_manifest().await?;
    let results = get_results().await?;

    assert_eq!(results.candidates.len(), manifest.candidates.len());
    // TODO: Check signature
    for ((i, c), p) in results.candidates.iter().enumerate().zip(psephoi.0.iter()) {
        let status = document()
            .get_element_by_id(&format!("tally-status-{i}"))
            .unwrap();
        status.set_text_content(Some("Verifying..."));

        let storage_key = format!("receipt_{}_{}", election, i);
        let receipt = gloo_storage::LocalStorage::get::<String>(&storage_key).ok();

        let path = format!("/{election}/nonces/{i}");
        let resp = Request::get(&path).send().await?;
        if !resp.ok() {
            bail!("{}", resp.text().await?);
        }
        let resp = resp.binary().await?;
        let hash = blake3::hash(&resp);
        assert_eq!(resp.len(), c.tally as usize * NONCE_LEN);
        assert_eq!(hash.as_bytes(), &c.nonces);
        let nonces =
            <[[u8; NONCE_LEN]]>::ref_from_bytes(&resp).map_err(|_| anyhow!("Bad nonce file"))?;
        for [x, y] in nonces.array_windows::<2>() {
            assert!(x < y); // Strictly less => no duplicates
        }
        let is_present = nonces.binary_search(&p.token).ok();
        let txt = match (receipt, is_present) {
            (Some(_), Some(_)) => "✅".into(),
            (None, None) => "✅".into(),
            (Some(receipt), None) => {
                format!("❌ The ballot counter threw away your vote!  Here's the proof: {receipt}")
            }
            (None, Some(i)) => {
                format!("❌ Someone stole your vote! It's at position {i} in the log")
            }
        };
        status.set_text_content(Some(&txt));
    }
    Ok(())
}

async fn audit() -> Result<()> {
    bail!("Not implemented yet")
}
