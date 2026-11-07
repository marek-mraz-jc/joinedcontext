//! Publishing and retiring a `wasm` App (AP-149, AP-150, AP-151, ADR-N-044): its server component
//! taken from the bundle the Portal checked and stored by digest where the WASM host loads it,
//! its shard placed and kept, its schema provisioned and migrated from the App's own repository
//! at the built commit; on retire its schema and its files exported first and dropped only after.

use std::io::Read;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::apps::apps_db::{app_id, AppsDb, Migration};
use crate::artifact_store::Client as Store;

/// The component inside an App's bundle (AP-151).
pub const COMPONENT_PATH: &str = ".jc/component.wasm";

/// The largest component the Portal stores, as the host's own limit.
pub const MAX_COMPONENT_BYTES: u64 = 64 << 20;

/// What a publish wrote, for the App's status (AP-149, AP-151).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// `sha256:<64 hex>` of the component.
    pub component: String,
    pub shard: u32,
}

/// The Portal's side of the server WASM Apps: the apps database and the bucket.
pub struct WasmApps {
    pub db: AppsDb,
    pub store: Arc<Store>,
    /// The bucket of `components/` and `apps/` (deployment's `apps` bucket).
    pub bucket: String,
}

/// `components/sha256-<hex>.wasm`, where the host fetches it (AP-151).
pub fn component_key(digest: &str) -> String {
    format!(
        "components/sha256-{}.wasm",
        digest.trim_start_matches("sha256:")
    )
}

/// The component of a checked bundle (`bundle.tar.gz`): the bytes at `.jc/component.wasm`, which
/// must be a WebAssembly component, never another entry and never larger than the cap.
pub fn component_of(bundle: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bundle));
    let entries = archive
        .entries()
        .map_err(|err| format!("the bundle is no tar.gz: {err}"))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("the bundle is no tar.gz: {err}"))?;
        let path = entry
            .path()
            .map_err(|err| format!("a bundle path: {err}"))?
            .to_string_lossy()
            .into_owned();
        if path.trim_start_matches("./") != COMPONENT_PATH {
            continue;
        }
        if entry.header().entry_type() != tar::EntryType::Regular {
            return Err(format!("{COMPONENT_PATH} in the bundle is no regular file"));
        }
        let mut bytes = Vec::new();
        entry
            .take(MAX_COMPONENT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| format!("{COMPONENT_PATH}: {err}"))?;
        if bytes.len() as u64 > MAX_COMPONENT_BYTES {
            return Err(format!(
                "{COMPONENT_PATH} is larger than {MAX_COMPONENT_BYTES} bytes"
            ));
        }
        // A component's preamble: the WebAssembly magic and the component layer (AP-151).
        if bytes.get(..8) != Some(&[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00]) {
            return Err(format!("{COMPONENT_PATH} is no WebAssembly component"));
        }
        return Ok(bytes);
    }
    Err(format!(
        "the bundle holds no {COMPONENT_PATH}: a wasm App's lane packs its server there (AP-151)"
    ))
}

/// An export file's name as one key segment: the App names its own tables, and a quoted name may
/// hold `/` or be `..`, which would put the file outside its export, under another App's prefix.
/// Anything but `[A-Za-z0-9_.-]` is written `%XX`, and a name starting with `.` gets a `%2E`.
pub fn export_file_name(file: &str) -> String {
    let mut out = String::with_capacity(file.len());
    for (i, b) in file.bytes().enumerate() {
        match b {
            b'.' if i == 0 => out.push_str("%2E"),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' => out.push(b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The migration files of `folder` among the paths of the built tree: `.sql` files directly in it.
pub fn migration_paths(tree: &[String], folder: &str) -> Vec<String> {
    let folder = folder.trim_matches('/');
    let mut paths: Vec<String> = tree
        .iter()
        .filter(|path| {
            path.strip_prefix(folder)
                .and_then(|rest| rest.strip_prefix('/'))
                .is_some_and(|file| !file.contains('/') && file.ends_with(".sql"))
        })
        .cloned()
        .collect();
    paths.sort();
    paths
}

impl WasmApps {
    /// Stores the component, places the App, provisions and migrates its schema; nothing of the
    /// App's status is written before all of it has held (AP-149, AP-151).
    pub async fn publish(
        &self,
        project: &str,
        name: &str,
        bundle: &[u8],
        migrations: &[Migration],
    ) -> Result<Published, String> {
        let component = component_of(bundle)?;
        let digest = format!("sha256:{:x}", Sha256::digest(&component));
        self.store
            .put_object(
                &self.bucket,
                &component_key(&digest),
                component,
                "store a component",
            )
            .await
            .map_err(|err| format!("the component could not be stored: {err}"))?;
        let (id, shard) = self
            .db
            .place(project, name)
            .await
            .map_err(|err| err.to_string())?;
        self.db
            .provision(&id, shard)
            .await
            .map_err(|err| err.to_string())?;
        self.db
            .migrate(&id, migrations)
            .await
            .map_err(|err| err.to_string())?;
        Ok(Published {
            component: digest,
            shard,
        })
    }

    /// Exports the App's schema and files to `apps/retired/<id>/<time>/` and only then drops the
    /// schema, the roles and the files (AP-150). A failed export leaves everything in place.
    /// An App never placed has nothing to retire.
    pub async fn retire(&self, project: &str, name: &str, at: &str) -> Result<bool, String> {
        let id = app_id(project, name);
        let Some(shard) = self.db.shard_of(&id).await.map_err(|err| err.to_string())? else {
            return Ok(false);
        };
        let export = format!("apps/retired/{id}/{at}");
        for (file, bytes) in self
            .db
            .export(&id)
            .await
            .map_err(|err| format!("the schema could not be exported: {err}"))?
        {
            self.store
                .put_object(
                    &self.bucket,
                    &format!("{export}/db/{}", export_file_name(&file)),
                    bytes,
                    "export a schema",
                )
                .await
                .map_err(|err| format!("the schema export could not be written: {err}"))?;
        }
        let prefix = format!("apps/{shard}/{id}/");
        let objects = self
            .store
            .list(&self.bucket, &prefix)
            .await
            .map_err(|err| format!("the App's files could not be listed: {err}"))?;
        for object in &objects {
            let relative = object.key.strip_prefix(&prefix).unwrap_or(&object.key);
            self.store
                .copy_object(
                    &self.bucket,
                    &object.key,
                    &format!("{export}/objects/{relative}"),
                )
                .await
                .map_err(|err| format!("the App's files could not be exported: {err}"))?;
        }
        // Both exports are written: now, and only now, the App's own data goes.
        self.db.retire(&id).await.map_err(|err| err.to_string())?;
        for object in &objects {
            self.store
                .delete_object(&self.bucket, &object.key)
                .await
                .map_err(|err| {
                    format!("the App's files were exported and could not all be deleted: {err}")
                })?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_export_file_name_never_leaves_its_folder() {
        use super::export_file_name;
        assert_eq!(export_file_name("notes.csv"), "notes.csv");
        assert_eq!(export_file_name("schema.sql"), "schema.sql");
        assert_eq!(
            export_file_name("../../apps/0/x.csv"),
            "%2E.%2F..%2Fapps%2F0%2Fx.csv"
        );
        assert_eq!(export_file_name("..csv"), "%2E.csv");
        assert!(!export_file_name("a/b\\c").contains(['/', '\\']));
    }

    use super::*;

    fn bundle(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, path, *bytes).expect("entry");
        }
        let raw = tar.into_inner().expect("tar");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &raw).expect("gz");
        gz.finish().expect("gz")
    }

    const COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00, 0x42];
    const MODULE: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

    #[test]
    fn the_component_is_taken_from_its_place_in_the_bundle_alone() {
        let good = bundle(&[
            ("index.html", b"<html>"),
            ("./.jc/component.wasm", COMPONENT),
        ]);
        assert_eq!(component_of(&good), Ok(COMPONENT.to_vec()));
        assert!(component_of(&bundle(&[("index.html", b"x")]))
            .unwrap_err()
            .contains("holds no .jc/component.wasm"));
        assert!(component_of(&bundle(&[(".jc/component.wasm", MODULE)]))
            .unwrap_err()
            .contains("no WebAssembly component"));
        assert!(
            component_of(&bundle(&[("x/.jc/component.wasm", COMPONENT)])).is_err(),
            "only the bundle's own .jc/"
        );
        assert!(component_of(b"not a gzip").is_err());
    }

    #[test]
    fn migrations_are_the_sql_files_directly_in_their_folder_in_name_order() {
        let tree: Vec<String> = [
            "server/src/lib.rs",
            "migrations/0002_b.sql",
            "migrations/0001_a.sql",
            "migrations/old/0000.sql",
            "migrations/README.md",
            "migrationsx/0003.sql",
        ]
        .iter()
        .map(|p| (*p).to_owned())
        .collect();
        assert_eq!(
            migration_paths(&tree, "migrations/"),
            ["migrations/0001_a.sql", "migrations/0002_b.sql"]
        );
        assert!(migration_paths(&tree, "none").is_empty());
    }

    #[test]
    fn a_component_is_kept_where_the_host_reads_it() {
        assert_eq!(component_key("sha256:ab"), "components/sha256-ab.wasm");
    }
}
