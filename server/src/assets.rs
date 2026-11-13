use axum::{
    extract::Path,
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use base64ct::{Base64UrlUnpadded, Encoding};
use linearize::{Linearize, StaticCopyMap, StaticMap};
use std::sync::LazyLock;

static WASM: &[u8] = include_bytes!(concat!(env!("PSEPHOI_CLIENT_DIR"), "/client_bg.wasm"));
static JS: &[u8] = include_bytes!(concat!(env!("PSEPHOI_CLIENT_DIR"), "/client.js"));
static WASM_GZ: &[u8] = include_bytes!(concat!(env!("PSEPHOI_CLIENT_DIR"), "/client_bg.wasm.gz"));
static JS_GZ: &[u8] = include_bytes!(concat!(env!("PSEPHOI_CLIENT_DIR"), "/client.js.gz"));
static CSS: &[u8] = include_bytes!("../../static/style.css");
static APP: &[u8] = include_bytes!("../../static/app.js");

// Static assets, served from /assets/
#[derive(Copy, Clone, Debug, Linearize)]
pub enum Asset {
    Css,
    Wasm,
    Bindings,
    Glue,
}

#[derive(Copy, Clone, Debug)]
struct AssetInfo {
    raw: &'static [u8],
    gzipped: Option<&'static [u8]>,
    ext: &'static str,
    mime: &'static str,
}

static ASSETS: &StaticCopyMap<Asset, AssetInfo> = StaticCopyMap::from_ref(&[
    AssetInfo {
        raw: CSS,
        gzipped: None,
        ext: "css",
        mime: "text/css",
    },
    AssetInfo {
        raw: WASM,
        gzipped: Some(WASM_GZ),
        ext: "wasm",
        mime: "application/wasm",
    },
    AssetInfo {
        raw: JS,
        gzipped: Some(JS_GZ),
        ext: "js",
        mime: "text/javascript",
    },
    AssetInfo {
        raw: APP,
        gzipped: None,
        ext: "js",
        mime: "text/javascript",
    },
]);

static ASSET_NAMES: LazyLock<StaticMap<Asset, String>> = LazyLock::new(|| {
    ASSETS.into_static_map().map_values(|info| {
        format!(
            "{}.{}",
            Base64UrlUnpadded::encode_string(blake3::hash(info.raw).as_bytes()),
            info.ext
        )
    })
});

pub fn asset_url(asset: Asset) -> String {
    format!("/assets/{}", ASSET_NAMES[asset])
}

pub async fn static_asset(headers: HeaderMap, Path(fname): Path<String>) -> Response {
    let Some(asset) = ASSET_NAMES
        .iter()
        .find_map(|(a, x)| (x == &fname).then_some(ASSETS[a]))
    else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body("No file with the given hash".into())
            .unwrap();
    };

    let accepts_gzip = headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .flat_map(|x| x.to_str().unwrap_or_default().split(','))
        .find(|x| x.trim_start().starts_with("gzip"))
        .is_some();

    let bldr = Response::builder()
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::CONTENT_TYPE, asset.mime)
        .header(header::VARY, "accept-encoding");

    if accepts_gzip && let Some(data_gz) = asset.gzipped {
        bldr.header(header::CONTENT_ENCODING, "gzip")
            .body(data_gz.into())
            .unwrap()
    } else {
        bldr.body(asset.raw.into()).unwrap()
    }
}
