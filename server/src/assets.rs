use axum::{
    extract::Path,
    http::{StatusCode, header},
    response::Response,
};
use base64ct::{Base64UrlUnpadded, Encoding};
use linearize::{Linearize, StaticCopyMap, StaticMap};
use std::sync::LazyLock;

static WASM: &[u8] = include_bytes!(concat!(env!("PSEPHOI_CLIENT_DIR"), "/client_bg.wasm"));
static JS: &[u8] = include_bytes!(concat!(env!("PSEPHOI_CLIENT_DIR"), "/client.js"));
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

static ASSETS: &StaticCopyMap<Asset, (&[u8], &str, &str)> = StaticCopyMap::from_ref(&[
    (CSS, "css", "text/css"),
    (WASM, "wasm", "application/wasm"),
    (JS, "js", "text/javascript"),
    (APP, "js", "text/javascript"),
]);

static ASSET_NAMES: LazyLock<StaticMap<Asset, String>> = LazyLock::new(|| {
    ASSETS.into_static_map().map_values(|(data, ext, _)| {
        format!(
            "{}.{ext}",
            Base64UrlUnpadded::encode_string(blake3::hash(data).as_bytes())
        )
    })
});

pub fn asset_url(asset: Asset) -> String {
    format!("/assets/{}", ASSET_NAMES[asset])
}

pub async fn static_asset(Path(fname): Path<String>) -> Response {
    let Some(asset) = ASSET_NAMES
        .iter()
        .find_map(|(a, x)| (x == &fname).then_some(a))
    else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body("No file with the given hash".into())
            .unwrap();
    };

    let (data, _, mime) = ASSETS[asset];

    Response::builder()
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::CONTENT_TYPE, mime)
        .body(data.into())
        .unwrap()
}
