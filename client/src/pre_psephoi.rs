use crate::{
    JsResultExt,
    fetch::{election, get_manifest},
};
use anyhow::Result;
use base64ct::{Base64UrlUnpadded, Encoding};
use blind_rsa_signatures::{BlindMessage, BlindingResult, Secret};
use psephoi::NONCE_LEN;
use tls_codec::{
    Deserialize, Serialize, TlsByteVecU16, TlsDeserialize, TlsSerialize, TlsSize, TlsVecU16,
};

#[derive(Clone, TlsSerialize, TlsDeserialize, TlsSize)]
pub struct PrePsephos {
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

pub async fn ensure_pre_psephoi_exist() -> Result<()> {
    let election = election().await?;
    let manifest = get_manifest().await?;

    let storage_key = format!("pre_psephoi_{}", election);
    if crate::local_storage()?
        .get_item(&storage_key)
        .js()?
        .is_some()
    {
        return Ok(());
    }

    let tokens = (0..manifest.candidates.len())
        .map(|_| {
            let token: [u8; NONCE_LEN] = rand::random();
            token
        })
        .collect::<Vec<_>>();
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
    let encoded = Base64UrlUnpadded::encode_string(&pre_psephoi.tls_serialize_detached()?);
    crate::local_storage()?
        .set_item(&storage_key, &encoded)
        .js()
}

pub async fn get_pre_psephoi() -> Result<Vec<PrePsephos>> {
    let election = election().await?;
    let storage_key = format!("pre_psephoi_{}", election);
    let Some(encoded) = crate::local_storage()?.get_item(&storage_key).js()? else {
        return Ok(vec![]);
    };
    let bytes = Base64UrlUnpadded::decode_vec(&encoded)?;
    Ok(TlsVecU16::<PrePsephos>::tls_deserialize_exact(&bytes)?.into_vec())
}
