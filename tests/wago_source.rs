//! HTTP-boundary tests for the Wago client against a local wiremock server.
//! The canned JSON mirrors the real live API contract (validated 2026-08-16
//! against ClassCodex, id rNkynwKa); tests/wago_live.rs exercises it against
//! the real API end-to-end.

use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wowctl::addon::ReleaseChannel;
use wowctl::error::WowctlError;
use wowctl::sources::AddonSource;
use wowctl::sources::wago::WagoSource;

const KEY: &str = "test-wago-key";

fn source(server: &MockServer) -> WagoSource {
    WagoSource::with_base_url(KEY.to_string(), server.uri()).unwrap()
}

/// Real search/detail release shape: `download_link` and
/// `supported_retail_patch`. Search release tiers do carry
/// `logical_timestamp` on the wire (the model no longer reads it); detail
/// release tiers never have it, so it's an optional argument.
fn stable_release(
    label: &str,
    created_at: &str,
    url: &str,
    logical_timestamp: Option<u64>,
) -> serde_json::Value {
    let mut v = serde_json::json!({
        "label": label,
        "download_link": url,
        "created_at": created_at,
        "stability": "stable",
        "supported_retail_patch": "11.2.0"
    });
    if let Some(ts) = logical_timestamp {
        v["logical_timestamp"] = serde_json::json!(ts);
    }
    v
}

/// Real `_recents` batch release shape: `link` (not `download_link`) and
/// `patch` (not `supported_retail_patch`), plus an `id` field we ignore, no
/// `logical_timestamp`.
fn recents_release(label: &str, created_at: &str, url: &str) -> serde_json::Value {
    serde_json::json!({
        "label": label,
        "link": url,
        "created_at": created_at,
        "id": "some-release-id",
        "patch": "11.2.0",
        "supported_patches": ["11.2.0"]
    })
}

fn search_body(server_uri: &str) -> serde_json::Value {
    // Real search items carry no `slug` field; `website_url` is the
    // fallback the client derives the slug from.
    serde_json::json!({
        "data": [
            {
                "id": "rNkynwKa",
                "display_name": "ClassCodex",
                "summary": "Class guide addon",
                "download_count": 1234,
                "website_url": "https://addons.wago.io/addons/classcodex",
                "releases": {
                    "stable": stable_release(
                        "1.2.0",
                        "2026-08-06T13:42:41.000000Z",
                        &format!("{server_uri}/dl/classcodex"),
                        Some(1755100000000000),
                    )
                }
            },
            {
                "id": "hidden01",
                "display_name": "HiddenAddon",
                "website_url": "https://addons.wago.io/addons/hidden-addon",
                "is_hidden_from_external": true,
                "releases": {}
            }
        ]
    })
}

fn detail_body(server_uri: &str) -> serde_json::Value {
    // Real detail responses key the releases object `recent_release`
    // (singular) and omit `is_hidden_from_external` (hidden addons are
    // filtered server-side); it's kept out of the base fixture here and
    // added explicitly by the test that exercises defensive filtering.
    serde_json::json!({
        "id": "rNkynwKa",
        "slug": "classcodex",
        "display_name": "ClassCodex",
        "summary": "Class guide addon",
        "download_count": 1234,
        "website_url": "https://addons.wago.io/addons/classcodex",
        "recent_release": {
            "stable": stable_release(
                "1.2.0",
                "2026-08-06T13:42:41.000000Z",
                &format!("{server_uri}/dl/classcodex"),
                None,
            ),
            "beta": stable_release(
                "1.3.0-beta",
                "2026-08-07T13:42:41.000000Z",
                &format!("{server_uri}/dl/classcodex-beta"),
                None,
            )
        }
    })
}

#[tokio::test]
async fn search_sends_bearer_and_filters_hidden() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(query_param("query", "classcodex"))
        .and(query_param("game_version", "retail"))
        .and(query_param("stability", "stable"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body(&server.uri())))
        .mount(&server)
        .await;

    let result = source(&server).search("classcodex", None).await.unwrap();

    // The hidden addon must be excluded (is_hidden_from_external).
    assert_eq!(result.addons.len(), 1);
    assert_eq!(result.addons[0].slug, "classcodex");
    assert_eq!(result.addons[0].source, "wago");
}

#[tokio::test]
async fn get_latest_version_stable_channel() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/rNkynwKa"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(query_param("game_version", "retail"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail_body(&server.uri())))
        .mount(&server)
        .await;

    let v = source(&server)
        .get_latest_version("rNkynwKa", ReleaseChannel::Stable)
        .await
        .unwrap();

    assert_eq!(v.version, "1.2.0");
    assert_eq!(v.file_id, None);
    assert_eq!(
        v.external_release_id,
        Some("2026-08-06T13:42:41.000000Z".to_string())
    );
    assert!(v.download_url.ends_with("/dl/classcodex"));
    assert!(v.dependencies.is_empty());
}

#[tokio::test]
async fn get_latest_version_beta_channel_picks_newer_beta() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/rNkynwKa"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail_body(&server.uri())))
        .mount(&server)
        .await;

    let v = source(&server)
        .get_latest_version("rNkynwKa", ReleaseChannel::Beta)
        .await
        .unwrap();

    assert_eq!(v.version, "1.3.0-beta");
}

#[tokio::test]
async fn hidden_addon_detail_is_not_found() {
    let server = MockServer::start().await;
    let mut body = detail_body(&server.uri());
    body["is_hidden_from_external"] = serde_json::json!(true);
    Mock::given(method("GET"))
        .and(path("/addons/rNkynwKa"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let err = source(&server)
        .get_latest_version("rNkynwKa", ReleaseChannel::Stable)
        .await
        .unwrap_err();
    assert!(matches!(err, WowctlError::AddonNotFound(_)));
}

#[tokio::test]
async fn unauthorized_maps_to_helpful_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = source(&server).search("anything", None).await.unwrap_err();
    match err {
        WowctlError::Unauthorized(msg) => {
            assert!(msg.contains("addons.wago.io/patreon"));
            assert!(msg.contains("Wago Addons Supporter"));
        }
        other => panic!("expected Unauthorized, got: {other}"),
    }
}

#[tokio::test]
async fn get_addon_by_slug_matches_exact_slug() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .and(query_param("query", "classcodex"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body(&server.uri())))
        .mount(&server)
        .await;

    let info = source(&server).get_addon_by_slug("classcodex").await.unwrap();
    assert_eq!(info.id, "rNkynwKa");
    assert_eq!(info.slug, "classcodex");
}

#[tokio::test]
async fn get_addon_by_slug_falls_back_to_detail_lookup() {
    let server = MockServer::start().await;
    // Search misses...
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .mount(&server)
        .await;
    // ...but the detail endpoint resolves the slug directly.
    Mock::given(method("GET"))
        .and(path("/addons/classcodex"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail_body(&server.uri())))
        .mount(&server)
        .await;

    let info = source(&server).get_addon_by_slug("classcodex").await.unwrap();
    assert_eq!(info.id, "rNkynwKa");
}

#[tokio::test]
async fn get_addon_by_slug_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/addons/nope"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let err = source(&server).get_addon_by_slug("nope").await.unwrap_err();
    assert!(matches!(err, WowctlError::AddonNotFound(_)));
}

#[tokio::test]
async fn recents_batch_maps_to_version_checks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/addons/_recents"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(body_json(serde_json::json!({
            "game_version": "retail",
            "addons": ["rNkynwKa"]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "addons": {
                "rNkynwKa": {
                    "id": "rNkynwKa",
                    "recent_releases": {
                        "stable": recents_release(
                            "1.3.0",
                            "2026-08-10T00:00:00.000000Z",
                            "https://example.com/1.3.0.zip",
                        )
                    }
                }
            }
        })))
        .mount(&server)
        .await;

    let checks = source(&server)
        .get_latest_versions_batch(&["rNkynwKa"], ReleaseChannel::Stable)
        .await
        .unwrap();

    let check = checks.get("rNkynwKa").unwrap();
    assert_eq!(check.version, "1.3.0");
    assert_eq!(check.file_id, None);
    assert_eq!(
        check.external_release_id,
        Some("2026-08-10T00:00:00.000000Z".to_string())
    );
}

#[tokio::test]
async fn download_sends_bearer_and_validates_zip() {
    let server = MockServer::start().await;
    let zip_bytes: &[u8] = b"PK\x03\x04wago-zip-payload";
    Mock::given(method("GET"))
        .and(path("/dl/classcodex"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/zip")
                .set_body_bytes(zip_bytes),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("classcodex.zip");
    let url = format!("{}/dl/classcodex", server.uri());

    source(&server).download(&url, &dest).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), zip_bytes);
}

#[tokio::test]
async fn resolve_dependencies_is_empty() {
    let server = MockServer::start().await;
    let deps = source(&server)
        .resolve_dependencies("rNkynwKa", ReleaseChannel::Stable)
        .await
        .unwrap();
    assert!(deps.is_empty());
}
