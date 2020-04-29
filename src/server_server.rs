use crate::{Data, MatrixResult};
use http::header::{HeaderValue, AUTHORIZATION};
use log::error;
use rocket::{get, post, put, response::content::Json, State};
use ruma_api::Endpoint;
use ruma_client_api::error::Error;
use ruma_federation_api::{v1::get_server_version, v2::get_server_keys};
use serde_json::json;
use std::{
    collections::BTreeMap,
    convert::TryFrom,
    time::{Duration, SystemTime},
};

pub async fn request_well_known(data: &crate::Data, destination: &str) -> Option<String> {
    let body: serde_json::Value = serde_json::from_str(&data
        .reqwest_client()
        .get(&format!(
            "https://{}/.well-known/matrix/server",
            destination
        ))
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?).ok()?;
    Some(body.get("m.server")?.as_str()?.to_owned())
}

pub async fn send_request<T: Endpoint>(
    data: &crate::Data,
    destination: String,
    request: T,
) -> Option<T::Response> {
    let mut http_request: http::Request<_> = request.try_into().unwrap();

    let actual_destination = "https://".to_owned() + &request_well_known(data, &destination).await.unwrap_or(destination.clone() + ":8448");
    *http_request.uri_mut() = (actual_destination + T::METADATA.path)
        .parse()
        .unwrap();

    let mut request_map = serde_json::Map::new();

    if !http_request.body().is_empty() {
        request_map.insert(
            "content".to_owned(),
            serde_json::to_value(http_request.body()).unwrap(),
        );
    };

    request_map.insert("method".to_owned(), T::METADATA.method.to_string().into());
    request_map.insert("uri".to_owned(), T::METADATA.path.into());
    request_map.insert("origin".to_owned(), data.hostname().into());
    request_map.insert("destination".to_owned(), destination.into());

    let mut request_json = request_map.into();
    ruma_signatures::sign_json(data.hostname(), data.keypair(), &mut request_json).unwrap();

    let signatures = request_json["signatures"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k, v.as_str().unwrap()));

    for s in signatures {
        http_request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!(
                "X-Matrix origin={},key=\"{}\",sig=\"{}\"",
                data.hostname(),
                s.0,
                s.1
            ))
            .unwrap(),
        );
    }

    let reqwest_response = data.reqwest_client().execute(http_request.into()).await;

    // Because reqwest::Response -> http::Response is complicated:
    match reqwest_response {
        Ok(mut reqwest_response) => {
            let status = reqwest_response.status();
            let mut http_response = http::Response::builder().status(status);
            let headers = http_response.headers_mut().unwrap();

            for (k, v) in reqwest_response.headers_mut().drain() {
                if let Some(key) = k {
                    headers.insert(key, v);
                }
            }

            let body = reqwest_response
                .bytes()
                .await
                .unwrap()
                .into_iter()
                .collect();
            Some(
                <T::Response>::try_from(http_response.body(body).unwrap())
                    .ok()
                    .unwrap(),
            )
        }
        Err(e) => {
            error!("{}", e);
            None
        }
    }
}

#[get("/.well-known/matrix/server")]
pub fn well_known_server(data: State<Data>) -> Json<String> {
    rocket::response::content::Json(
        json!({ "m.server": "matrixtesting.koesters.xyz:14004"}).to_string(),
    )
}

#[get("/_matrix/federation/v1/version")]
pub fn get_server_version() -> MatrixResult<get_server_version::Response, Error> {
    MatrixResult(Ok(get_server_version::Response {
        server: Some(get_server_version::Server {
            name: Some("Conduit".to_owned()),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    }))
}

#[get("/_matrix/key/v2/server")]
pub fn get_server_keys(data: State<Data>) -> Json<String> {
    let mut verify_keys = BTreeMap::new();
    verify_keys.insert(
        format!("ed25519:{}", data.keypair().version()),
        get_server_keys::VerifyKey {
            key: base64::encode_config(data.keypair().public_key(), base64::STANDARD_NO_PAD),
        },
    );
    let mut response = serde_json::from_slice(
        http::Response::try_from(get_server_keys::Response {
            server_name: data.hostname().to_owned(),
            verify_keys,
            old_verify_keys: BTreeMap::new(),
            signatures: BTreeMap::new(),
            valid_until_ts: SystemTime::now() + Duration::from_secs(60 * 2),
        })
        .unwrap()
        .body(),
    )
    .unwrap();
    ruma_signatures::sign_json(data.hostname(), data.keypair(), &mut response).unwrap();
    Json(response.to_string())
}

#[get("/_matrix/key/v2/server/<_key_id>")]
pub fn get_server_keys_deprecated(data: State<Data>, _key_id: String) -> Json<String> {
    get_server_keys(data)
}
