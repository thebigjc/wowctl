//! HTTP-boundary tests for the CurseForge client against a local wiremock server.

use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wowctl::addon::ReleaseChannel;
use wowctl::sources::AddonSource;
use wowctl::sources::curseforge::CurseForgeSource;

fn search_body() -> serde_json::Value {
    serde_json::json!({
        "data": [{
            "id": 65387,
            "name": "WeakAuras",
            "slug": "weakauras-2",
            "summary": "A powerful framework",
            "downloadCount": 1000000.0,
            "latestFiles": [],
            "links": {"websiteUrl": "https://www.curseforge.com/wow/addons/weakauras-2"},
            "latestFilesIndexes": []
        }],
        "pagination": {"index": 0, "pageSize": 20, "resultCount": 1, "totalCount": 1}
    })
}

fn files_body() -> serde_json::Value {
    serde_json::json!({
        "data": [{
            "id": 5877543,
            "displayName": "WeakAuras 5.12.8",
            "fileName": "WeakAuras-5.12.8.zip",
            "downloadUrl": "https://example.com/wa.zip",
            "fileLength": 500000,
            "gameVersions": ["11.1.0"],
            "dependencies": [],
            "fileDate": "2026-03-01T00:00:00Z",
            "releaseType": 1,
            "modules": [{"name": "WeakAuras", "fingerprint": 1}]
        }]
    })
}

#[tokio::test]
async fn search_hits_api_with_key_and_maps_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mods/search"))
        .and(header("x-api-key", "test-key"))
        .and(query_param("gameId", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body()))
        .mount(&server)
        .await;

    let source = CurseForgeSource::with_base_url("test-key".to_string(), server.uri()).unwrap();
    let result = source.search("weakauras", None).await.unwrap();

    assert_eq!(result.addons.len(), 1);
    assert_eq!(result.addons[0].slug, "weakauras-2");
    assert_eq!(result.addons[0].source, "curseforge");
    assert_eq!(result.total_count, 1);
}

#[tokio::test]
async fn get_latest_version_picks_retail_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mods/65387/files"))
        .and(header("x-api-key", "test-key"))
        .and(query_param("gameVersionTypeId", "517"))
        .respond_with(ResponseTemplate::new(200).set_body_json(files_body()))
        .mount(&server)
        .await;

    let source = CurseForgeSource::with_base_url("test-key".to_string(), server.uri()).unwrap();
    let v = source
        .get_latest_version("65387", ReleaseChannel::Stable)
        .await
        .unwrap();

    assert_eq!(v.file_id, Some(5877543));
    assert_eq!(v.external_release_id, None);
    assert_eq!(v.version, "5.12.8");
    assert_eq!(v.download_url, "https://example.com/wa.zip");
    assert_eq!(v.modules, vec!["WeakAuras".to_string()]);
}

#[tokio::test]
async fn download_writes_valid_zip() {
    let server = MockServer::start().await;
    let zip_bytes: &[u8] = b"PK\x03\x04rest-of-zip-payload";
    Mock::given(method("GET"))
        .and(path("/files/addon.zip"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/zip")
                .set_body_bytes(zip_bytes),
        )
        .mount(&server)
        .await;

    let source = CurseForgeSource::with_base_url("test-key".to_string(), server.uri()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("addon.zip");
    let url = format!("{}/files/addon.zip", server.uri());

    let written = source.download(&url, &dest).await.unwrap();
    assert_eq!(written, dest);
    assert_eq!(std::fs::read(&dest).unwrap(), zip_bytes);
}

#[tokio::test]
async fn download_rejects_html_error_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files/broken.zip"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string("<html>error</html>"),
        )
        .mount(&server)
        .await;

    let source = CurseForgeSource::with_base_url("test-key".to_string(), server.uri()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("broken.zip");
    let url = format!("{}/files/broken.zip", server.uri());

    assert!(source.download(&url, &dest).await.is_err());
    assert!(!dest.exists());
}
