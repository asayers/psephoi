use anyhow::{Context, Result, anyhow, bail, ensure};
use base64ct::{Base64UrlUnpadded, Encoding};
use blake3::Hash;
use blind_rsa_signatures::{BlindMessage, BlindingResult, Secret};
use gloo_events::EventListener;
use gloo_net::http::Request;
use gloo_storage::Storage;
use gloo_utils::document;
use gloo_utils::window;
use jiff::Timestamp;
use log::info;
use maud::html;
use psephoi::{
    Election, ElectionManifest, ElectionResults, NONCE_LEN, Psephoi, Psephos, PublicKey,
    RegRequest, RegResponse, RegToken, RegWhileVoting, SIG_LEN, VoteRequest, VoteResponse,
    render_manifest,
};
use rand::rng;
use rand::seq::SliceRandom;
use tls_codec::{
    Deserialize, Serialize, TlsByteVecU16, TlsDeserialize, TlsSerialize, TlsSize, TlsVecU16,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use zerocopy::FromBytes;

#[wasm_bindgen(start)]
pub async fn init() -> Result<(), JsError> {
    init_().await.map_err(|e| JsError::new(&format!("{e:#}")))
}

pub async fn init_() -> Result<()> {
    console_log::init_with_level(log::Level::Debug).unwrap();
    console_error_panic_hook::set_once();
    info!("Psephoi client is up and running!");
    let manifest = get_manifest().await?;
    document().set_title(manifest.name());

    render().await?;

    {
        let register_box = document().query_selector("#register").js()?.unwrap();
        let button = register_box.query_selector("button").js()?.unwrap();
        let status_area = register_box.query_selector("p.status").js()?.unwrap();

        {
            let button = button.clone();
            let status_area = status_area.clone();
            EventListener::new(&button.clone(), "click", move |_| {
                status_area.set_text_content(Some("Registering..."));
                button.set_attribute("disabled", "").unwrap();
                let button = button.clone();
                let status_area = status_area.clone();
                spawn_local(async move {
                    match register().await {
                        Ok(_) => {
                            status_area.set_text_content(Some("Registered"));
                            vote_prep().await.unwrap();
                        }
                        Err(e) => {
                            status_area.set_text_content(Some(&format!("{e:#}")));
                            button.remove_attribute("disabled").unwrap();
                        }
                    }
                });
            })
            .forget();
        }

        status_area.set_text_content(Some("Preparing..."));
        match reg_prep().await {
            Ok(()) => {
                status_area.set_text_content(Some("Ready to register"));
                button.remove_attribute("disabled").js()?;
            }
            Err(e) => status_area.set_text_content(Some(&format!("{e:#}"))),
        }
    }

    {
        let vote_box = document().query_selector("#vote").js()?.unwrap();
        let button = vote_box.query_selector("button").js()?.unwrap();
        let status_area = vote_box.query_selector("p.status").js()?.unwrap();

        {
            let button = button.clone();
            let status_area = status_area.clone();
            EventListener::new(&button.clone(), "click", move |_| {
                status_area.set_text_content(Some("Submitting vote..."));
                button.set_attribute("disabled", "").unwrap();
                let button = button.clone();
                let status_area = status_area.clone();
                spawn_local(async move {
                    match vote().await {
                        Ok(cs) => status_area
                            .set_text_content(Some(&format!("Submitted vote for {cs:?}"))),
                        Err(e) => {
                            status_area.set_text_content(Some(&format!("{e:#}")));
                            button.remove_attribute("disabled").unwrap();
                        }
                    }
                });
            })
            .forget();
        }

        vote_prep().await?;
    }

    {
        let tally_box = document().query_selector("#tally").js()?.unwrap();
        let fetch_button = document().get_element_by_id("btn-fetch").unwrap();
        let verify_button = document().get_element_by_id("btn-verify").unwrap();
        let status_area = tally_box.query_selector("p.status").js()?.unwrap();
        status_area.set_text_content(Some(""));

        {
            let status_area = status_area.clone();
            let fetch_button = fetch_button.clone();
            EventListener::new(&fetch_button.clone(), "click", move |_| {
                status_area.set_text_content(Some("Getting tally..."));
                fetch_button.set_attribute("disabled", "").unwrap();
                let status_area = status_area.clone();
                spawn_local(async move {
                    match tally().await {
                        Ok(x) => status_area.set_text_content(Some(&x)),

                        Err(e) => {
                            status_area.set_text_content(Some(&format!("{e:#}")));
                        }
                    }
                });
            })
            .forget();
        }

        {
            let status_area = status_area.clone();
            EventListener::new(&verify_button.clone(), "click", move |_| {
                status_area.set_text_content(Some("Downloading full results..."));
                verify_button.set_attribute("disabled", "").unwrap();
                let status_area = status_area.clone();
                spawn_local(async move {
                    match verify_tally().await {
                        Ok(()) => status_area.set_text_content(Some("Verified tally")),
                        Err(e) => {
                            status_area.set_text_content(Some(&format!("{e:#}")));
                        }
                    }
                });
            })
            .forget();
        }

        if Timestamp::now() > get_manifest().await?.voting_period().end {
            status_area.set_text_content(Some("Ready"));
            fetch_button.remove_attribute("disabled").js()?;
        } else {
            status_area.set_text_content(Some("The election hasn't finished yet"))
        }
    }
    Ok(())
}

async fn get_election() -> Result<Election> {
    let txt = window().location().pathname().js()?;
    let bytes = Base64UrlUnpadded::decode_vec(txt.trim_start_matches('/'))
        .context("election ID should be base64url-encoded")?;
    let bytes = bytes
        .try_into()
        .ok()
        .context("election ID should be 43 characters long")?;
    let hash = Hash::from_bytes(bytes);
    Ok(Election(hash))
}

async fn render() -> Result<()> {
    let election = get_election().await?;
    let manifest = get_manifest().await?;
    let mut candidates = manifest
        .candidates
        .iter()
        .enumerate()
        .map(|(i, x)| (i, x.name()))
        .collect::<Vec<_>>();
    candidates.shuffle(&mut rng());
    // TODO: Highlight winner(s) in tally
    let markup = html! {
        header { h1 {
            span .election_name { (manifest.name()) }
            br;
            span .emoji { (election.emoji()) }
        } }
        main {
            fieldset #register {
                legend { "Register" }
                p .status { "Loading..." }
                button #btn-register disabled { "Register" }
            }
            fieldset #vote {
                legend { "Vote" }
                @for (i, c) in &candidates {
                    div {
                        input type="checkbox" id={"candidate-" (i)} name=(c)
                        label for={"candidate-" (i)} { (c) }
                    }
                }
                p .status { "Loading..." }
                button #btn-vote disabled { "Vote" }
            }
            fieldset #tally {
                legend { "Results" }
                @for (i, c) in &candidates {
                    div {
                        input readonly id={"tally-" (i)}
                        label for={"tally-" (i)} { (c) }
                        span id={"tally-status-" (i)};
                    }
                }
                p .status { "Loading..." }
                button #btn-fetch disabled { "Fetch" }
                button #btn-verify disabled { "Verify" }
                button #btn-full-verify disabled { "Fully verify" }
            }
            (render_manifest(&manifest))
        }
    };
    document()
        .query_selector("body")
        .js()?
        .unwrap()
        .set_inner_html(&markup.into_string());
    Ok(())
}

async fn get_manifest() -> Result<ElectionManifest> {
    let election = get_election().await?;
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
}

async fn retrive_psephoi() -> Result<Option<Psephoi>> {
    let election = get_election().await?;
    let storage_key = format!("psephoi_{}", election);
    gloo_storage::LocalStorage::get::<String>(&storage_key)
        .ok()
        .map(|encoded| {
            let bytes = Base64UrlUnpadded::decode_vec(&encoded)?;
            Ok(Psephoi::tls_deserialize_exact(&bytes)?)
        })
        .transpose()
}

#[derive(Clone, TlsSerialize, TlsDeserialize, TlsSize)]
struct PrePsephos {
    pub unblind_message: [u8; NONCE_LEN],
    pub blind_message: TlsByteVecU16,
    pub secret: TlsByteVecU16,
    pub msg_randomizer: Option<[u8; 32]>,
}

impl PrePsephos {
    fn new(token: [u8; NONCE_LEN], value: BlindingResult) -> Self {
        PrePsephos {
            unblind_message: token,
            blind_message: value.blind_message.0.into(),
            secret: value.secret.0.into(),
            msg_randomizer: value.msg_randomizer.map(|x| x.0),
        }
    }
}

impl From<PrePsephos> for BlindingResult {
    fn from(value: PrePsephos) -> Self {
        BlindingResult {
            blind_message: BlindMessage(value.blind_message.into_vec()),
            secret: Secret(value.secret.into_vec()),
            msg_randomizer: value
                .msg_randomizer
                .map(blind_rsa_signatures::MessageRandomizer),
        }
    }
}

pub async fn reg_prep() -> Result<()> {
    let election = get_election().await?;
    let manifest = get_manifest().await?;

    ensure!(
        retrive_psephoi().await?.is_none(),
        "You're already registered for this election"
    );

    ensure!(
        Timestamp::now() < manifest.registration_closes(),
        "Registration is closed"
    );

    let storage_key = format!("pre_psephoi_{}", election);
    if gloo_storage::LocalStorage::get::<String>(&storage_key).is_ok() {
        info!("Pre-psephoi for this election already in local storage");
        return Ok(());
    }

    let tokens = (0..manifest.candidates.len())
        .map(|_| {
            let token: [u8; NONCE_LEN] = rand::random();
            token
        })
        .collect::<Vec<_>>();
    info!("Generated {} tokens", manifest.candidates.len());
    let pre_psephoi = manifest
        .candidates
        .iter()
        .zip(&tokens)
        .map(|(c, token)| {
            let pk = c.pubkey();
            let res = pk.blind(&mut blind_rsa_signatures::DefaultRng, token)?;
            Ok(PrePsephos::new(*token, res))
        })
        .collect::<Result<TlsVecU16<_>>>()?;
    info!("Blinded the tokens");
    let encoded = Base64UrlUnpadded::encode_string(&pre_psephoi.tls_serialize_detached()?);
    gloo_storage::LocalStorage::set(storage_key, encoded).unwrap();
    Ok(())
}

pub async fn vote_prep() -> Result<()> {
    let manifest = get_manifest().await?;
    let vote_box = document().query_selector("#vote").js()?.unwrap();
    let button = vote_box.query_selector("button").js()?.unwrap();
    let status_area = vote_box.query_selector("p.status").js()?.unwrap();
    let now = Timestamp::now();
    let period = manifest.voting_period();
    if now < period.start {
        status_area.set_text_content(Some("Voting not open yet"));
    } else if now > period.end {
        status_area.set_text_content(Some("Voting is closed"))
    } else if retrive_psephoi().await?.is_some() {
        status_area.set_text_content(Some("Ready to vote.  Vote for as many as you like."));
        button.remove_attribute("disabled").js()?;
    } else if manifest.reg_while_voting == RegWhileVoting::Allow {
        // TODO: Just auto-register when they hit "vote"
        status_area.set_text_content(Some("You need to register first"));
    } else {
        status_area.set_text_content(Some("You missed the registration window"));
    }
    Ok(())
}

pub async fn register() -> Result<()> {
    let election = get_election().await?;

    ensure!(
        retrive_psephoi().await?.is_none(),
        "You've already got psephoi for this election"
    );

    let storage_key = format!("pre_psephoi_{}", election);
    let encoded = gloo_storage::LocalStorage::get::<String>(&storage_key)?;
    let bytes = Base64UrlUnpadded::decode_vec(&encoded)?;
    let pre_psephoi = TlsVecU16::<PrePsephos>::tls_deserialize_exact(&bytes)?;

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

pub async fn vote() -> Result<Vec<String>> {
    let election = get_election().await?;
    let manifest = get_manifest().await?;
    let psephoi = retrive_psephoi().await?.context("Not registered")?;
    let mut cs = Vec::new();
    for ((i, c), p) in manifest.candidates.iter().enumerate().zip(psephoi.0.iter()) {
        let checkbox = document()
            .get_element_by_id(&format!("candidate-{i}"))
            .unwrap();
        let checkbox = checkbox
            .dyn_into::<HtmlInputElement>()
            .map_err(|_| anyhow!("candidate-{i} isn't an <input>"))?;
        if checkbox.checked() {
            let storage_key = format!("receipt_{}_{}", election, i);
            cs.push(c.name().to_owned());
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
    Ok(cs)
}

pub async fn tally() -> Result<String> {
    let election = get_election().await?;
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
    let mut total = 0;
    for (i, c) in results.candidates.iter().enumerate() {
        let input = document().get_element_by_id(&format!("tally-{i}")).unwrap();
        let input = input
            .dyn_into::<HtmlInputElement>()
            .map_err(|_| anyhow!("tally-{i} isn't an <input>"))?;
        input.set_value(&c.tally.to_string());
        total += c.tally;
    }
    let verify_button = document().get_element_by_id("btn-verify").unwrap();
    let full_verify_button = document().get_element_by_id("btn-full-verify").unwrap();
    verify_button.remove_attribute("disabled").js()?;
    full_verify_button.remove_attribute("disabled").js()?;
    verify_button
        .set_attribute(
            "title",
            &format!("requires downloading {}B", total as usize * NONCE_LEN),
        )
        .js()?;
    full_verify_button
        .set_attribute(
            "title",
            &format!(
                "requires downloading {}B",
                total as usize * (NONCE_LEN + SIG_LEN)
            ),
        )
        .js()?;
    Ok("Unverified".into())
}

pub async fn verify_tally() -> Result<()> {
    let election = get_election().await?;
    let psephoi = retrive_psephoi().await?.context("Not registered")?;
    let manifest = get_manifest().await?;

    let path = format!("/{election}/results");
    // TODO: Re-use from tally()
    info!("Requesting results from {path}");
    let resp = Request::get(&path).send().await?;
    if !resp.ok() {
        let err = resp.text().await?;
        bail!("{err}");
    }
    let resp = resp.binary().await?;
    let results = ElectionResults::from_bytes(&resp)?;
    assert_eq!(results.election, *election.0.as_bytes());
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
