#![forbid(unsafe_code)]
//! `apply_branding_file`: the deployment's `global.branding` file (the Portal's
//! `JC_BRANDING_FILE`) becomes every realm's branding, its SVG logo read from beside it; an
//! unsafe logo and a path out of the file's folder are never served.

use kaisatsu_core::Store;
use std::path::PathBuf;
use std::sync::Arc;

mod common;
use common::*;

const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#0000bf"/></svg>"##;

/// A fresh folder holding `branding.yaml` with `yaml` and the given files beside it.
fn folder(name: &str, yaml: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kaisatsu-branding-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("branding.yaml"), yaml).unwrap();
    for (file, body) in files {
        std::fs::write(dir.join(file), body).unwrap();
    }
    dir.join("branding.yaml")
}

const SMARTCITY: &str = r##"instanceName: joinedcontext demo
organisation: joinedcontext demo
licenseDefault: CC-BY-4.0
logo: logo.svg
colours:
  primary: "#0000bf"
  secondary: "#0072c6"
  accent: "#ffe977"
  background: "#ffffff"
  text: "#1a1a1a"
"##;

#[tokio::test]
async fn the_file_brands_every_realm_with_its_logo() {
    let (_, store, _) = setup_dev_realm_state().await;
    let store: Arc<dyn Store> = store;
    let path = folder("ok", SMARTCITY, &[("logo.svg", LOGO)]);
    let n = kaisatsu_server::apply_branding_file(&store, &path)
        .await
        .unwrap();
    assert_eq!(n, store.list_realms().await.unwrap().len());
    let b = store.get_branding("dev", None).await.unwrap().unwrap();
    assert_eq!(b.organisation, "joinedcontext demo");
    assert_eq!(b.colours.primary, "#0000bf");
    assert_eq!(b.logo_svg, LOGO);
    assert_eq!(b.org_id, None);
}

#[tokio::test]
async fn an_unsafe_logo_or_a_path_out_of_the_folder_is_never_read() {
    let (_, store, _) = setup_dev_realm_state().await;
    let store: Arc<dyn Store> = store;
    let script = r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
    let path = folder("unsafe", SMARTCITY, &[("logo.svg", script)]);
    kaisatsu_server::apply_branding_file(&store, &path)
        .await
        .unwrap();
    let b = store.get_branding("dev", None).await.unwrap().unwrap();
    assert!(b.logo_svg.is_empty(), "{}", b.logo_svg);

    let escape = SMARTCITY.replace("logo: logo.svg", "logo: ../outside.svg");
    let path = folder("escape", &escape, &[]);
    std::fs::write(
        path.parent().unwrap().parent().unwrap().join("outside.svg"),
        LOGO,
    )
    .unwrap();
    kaisatsu_server::apply_branding_file(&store, &path)
        .await
        .unwrap();
    let b = store.get_branding("dev", None).await.unwrap().unwrap();
    assert!(b.logo_svg.is_empty() && b.logo.is_empty());
}

#[tokio::test]
async fn a_broken_file_is_an_error_and_changes_nothing() {
    let (_, store, _) = setup_dev_realm_state().await;
    let store: Arc<dyn Store> = store;
    let before = store.get_branding("dev", None).await.unwrap();
    let path = folder("broken", "colours: [not, a, map]\n", &[]);
    assert!(
        kaisatsu_server::apply_branding_file(&store, &path)
            .await
            .is_err()
    );
    let missing = folder("missing", SMARTCITY, &[]);
    assert!(
        kaisatsu_server::apply_branding_file(&store, &missing)
            .await
            .is_err()
    );
    assert_eq!(store.get_branding("dev", None).await.unwrap(), before);
}
