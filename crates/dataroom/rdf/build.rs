//! Embeds each directory of `../profiles` with the files its manifest names.

use std::error::Error;
use std::fmt::Write as _;
use std::path::Path;

fn main() -> Result<(), Box<dyn Error>> {
    let root = Path::new(&std::env::var("CARGO_MANIFEST_DIR")?)
        .join("..")
        .canonicalize()?;
    let profiles = root.join("profiles");
    println!("cargo:rerun-if-changed={}", profiles.display());
    let mut dirs = std::fs::read_dir(&profiles)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()?;
    dirs.retain(|path| path.is_dir());
    dirs.sort();

    let mut out = String::from(
        "type Embedded = (&'static str, &'static str, &'static [(&'static str, &'static str)]);\n\
         const EMBEDDED: &[Embedded] = &[\n",
    );
    for dir in dirs {
        let name = dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("a profile directory's name is not UTF-8")?;
        let path = dir.join("profile.json");
        let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let list = |key: &str| manifest[key].as_array().cloned().unwrap_or_default();
        let files = list("shapes")
            .into_iter()
            .chain(list("contexts").into_iter().map(|c| c["file"].clone()));
        let mut embedded = String::new();
        for file in files {
            let file = file
                .as_str()
                .ok_or_else(|| format!("{}: a file is not a string", path.display()))?
                .to_owned();
            let full = dir
                .join(&file)
                .canonicalize()
                .map_err(|e| format!("{}: {file}: {e}", path.display()))?;
            if !full.starts_with(&root) {
                return Err(format!("{}: {file} leaves crates/dataroom", path.display()).into());
            }
            println!("cargo:rerun-if-changed={}", full.display());
            write!(embedded, "({file:?}, include_str!({full:?})), ")?;
        }
        writeln!(
            out,
            "    ({name:?}, include_str!({path:?}), &[{embedded}]),"
        )?;
    }
    out.push_str("];\n");
    std::fs::write(
        Path::new(&std::env::var("OUT_DIR")?).join("profiles.rs"),
        out,
    )?;
    Ok(())
}
