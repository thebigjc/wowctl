//! Live acceptance test against the real Wago API. Ignored by default.
//! Run with a real key:
//!   WOWCTL_WAGO_ACCESS_KEY=<key> cargo test --test wago_live -- --ignored --nocapture

use wowctl::addon::ReleaseChannel;
use wowctl::sources::AddonSource;
use wowctl::sources::wago::WagoSource;

#[tokio::test]
#[ignore = "hits the live Wago API; requires WOWCTL_WAGO_ACCESS_KEY"]
async fn classcodex_search_resolve_and_download() {
    let key = std::env::var("WOWCTL_WAGO_ACCESS_KEY")
        .expect("set WOWCTL_WAGO_ACCESS_KEY to run the live test");
    let source = WagoSource::new(key).unwrap();

    // Slug resolution (user story 1); issue #8 records the expected Wago ID.
    let info = source.get_addon_by_slug("classcodex").await.unwrap();
    assert_eq!(info.id, "rNkynwKa");
    assert_eq!(info.source, "wago");

    // Latest stable release resolves with a download link and release identity.
    let v = source
        .get_latest_version(&info.id, ReleaseChannel::Stable)
        .await
        .unwrap();
    assert!(!v.version.is_empty());
    assert!(v.external_release_id.is_some());

    // Signed download with Bearer auth yields a real zip (user story 18).
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("classcodex.zip");
    source.download(&v.download_url, &dest).await.unwrap();
    let bytes = std::fs::read(&dest).unwrap();
    assert_eq!(&bytes[..4], b"PK\x03\x04");

    // Merged-search visibility (user story 6).
    let results = source.search("classcodex", None).await.unwrap();
    assert!(results.addons.iter().any(|a| a.slug == "classcodex"));

    // Batch recents contract (user story 5).
    let checks = source
        .get_latest_versions_batch(&[info.id.as_str()], ReleaseChannel::Stable)
        .await
        .unwrap();
    assert!(checks.contains_key(&info.id));
}
