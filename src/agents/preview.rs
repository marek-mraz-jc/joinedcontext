//! The preview of a code run: one document (Architecture/20 §5, SDK-16, AP-50).
//!
//! The run's interface files, transpiled, and the SDK runtime embedded in the Portal are all
//! `data:` modules behind one inline import map; the stylesheets are inlined; the SDK reads its
//! configuration from `#jc-config` with the bridge transport, so the document holds no rows, no
//! token and no cookie. The policy allows the two inline scripts by hash, the `data:` modules,
//! and a connection to the basemap route only.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use rust_embed::RustEmbed;
use serde::Deserialize;

use super::kit;
use super::transpile::{self, Problem};

/// `sdk/dist/runtime`, built by `vite.runtime.config.ts`; empty in a plain `cargo test`.
#[derive(RustEmbed)]
#[folder = "sdk/dist/runtime"]
struct Runtime;

/// The template application every code run starts from (SDK-10).
#[derive(RustEmbed)]
#[folder = "sdk/template"]
#[exclude = "dist/*"]
#[exclude = "node_modules/*"]
// The runner image installs from it (AP-127); an app never installs, and a run's model reads the
// template as its context, where 2 000 lines of resolutions would crowd out the code.
#[exclude = "pnpm-lock.yaml"]
pub struct Template;

/// What a `wasm` run adds over [`Template`] (T-3575): the server crate on `jc-app-sdk`, its lock,
/// its migration, the interface's client of it and the workflow that builds the component.
#[derive(RustEmbed)]
#[folder = "sdk/template-wasm"]
#[exclude = "server/target/*"]
pub struct TemplateWasm;

/// The entry the Portal owns (SDK-11): whatever the run holds, the preview starts here.
pub const MAIN: &str = "src/main.tsx";
const ENTRY: &str = "import \"@app/src/main.tsx\";";

#[derive(Deserialize)]
struct Manifest {
    names: BTreeMap<String, String>,
}

/// The runtime's import-map entries, already written as `data:` URLs, and its stylesheet.
struct Embedded {
    imports: serde_json::Map<String, serde_json::Value>,
    css: String,
}

static EMBEDDED: LazyLock<Option<Embedded>> = LazyLock::new(|| {
    let manifest: Manifest = serde_json::from_slice(&Runtime::get("runtime.json")?.data).ok()?;
    let mut imports = serde_json::Map::new();
    for (name, file) in manifest.names {
        let code = Runtime::get(&file)?;
        imports.insert(
            name,
            serde_json::Value::String(data_url(&String::from_utf8_lossy(&code.data))),
        );
    }
    let css = Runtime::get("sdk.css")
        .map(|file| String::from_utf8_lossy(&file.data).into_owned())
        .unwrap_or_default();
    Some(Embedded { imports, css })
});

/// The template's files, path to text.
pub fn template_files() -> BTreeMap<String, String> {
    Template::iter()
        .filter_map(|path| {
            let file = Template::get(&path)?;
            Some((
                path.into_owned(),
                String::from_utf8_lossy(&file.data).into_owned(),
            ))
        })
        .collect()
}

/// The files a `wasm` run starts from: the template's, with [`TemplateWasm`]'s over them.
pub fn wasm_template_files() -> BTreeMap<String, String> {
    let mut files = template_files();
    files.extend(TemplateWasm::iter().filter_map(|path| {
        let file = TemplateWasm::get(&path)?;
        Some((
            path.into_owned(),
            String::from_utf8_lossy(&file.data).into_owned(),
        ))
    }));
    files
}

#[derive(Debug)]
pub enum Refusal {
    /// The Portal was built without `sdk/dist/runtime`.
    NoRuntime,
    /// The files do not build: every problem, with file and line (SDK-12, SDK-14).
    Problems(Vec<Problem>),
}

#[derive(Debug)]
pub struct Document {
    pub html: String,
    pub csp: String,
}

/// The document for `files` (the run's `src/**` and `functions/**`), with `config` as the SDK
/// configuration and `basemap` as the one address it may connect to: a route prefix ending in `/`.
pub fn document(
    files: &BTreeMap<String, String>,
    title: &str,
    config: &serde_json::Value,
    basemap: Option<&str>,
) -> Result<Document, Refusal> {
    let mut files = files.clone();
    if let Some(main) = Template::get(MAIN) {
        files.insert(
            MAIN.to_owned(),
            String::from_utf8_lossy(&main.data).into_owned(),
        );
    }
    let project = transpile::transpile(&files);
    if !project.problems.is_empty() {
        return Err(Refusal::Problems(project.problems));
    }
    let embedded = EMBEDDED.as_ref().ok_or(Refusal::NoRuntime)?;
    let mut imports = embedded.imports.clone();
    for (name, code) in &project.modules {
        imports.insert(name.clone(), serde_json::Value::String(data_url(code)));
    }
    let import_map = script_json(&serde_json::json!({ "imports": imports }));
    let styles: String = std::iter::once(embedded.css.as_str())
        .chain(project.styles.iter().map(String::as_str))
        .map(|css| format!("<style>{}</style>\n", css.replace("</", "<\\/")))
        .collect();
    let worker = kit::worker()
        .map(|worker| format!("<script id=\"kit-worker\" type=\"text/plain\">{worker}</script>\n"))
        .unwrap_or_default();
    let html = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title}</title>\n{styles}<script type=\"importmap\">{import_map}</script>\n</head>\n\
         <body>\n<div id=\"root\"></div>\n\
         <script id=\"jc-config\" type=\"application/json\">{config}</script>\n{worker}\
         <script type=\"module\">{ENTRY}</script>\n</body>\n</html>\n",
        title = title
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;"),
        config = script_json(config),
    );
    let csp = content_security_policy(&import_map, basemap);
    Ok(Document { html, csp })
}

/// The preview's policy. The document is agent-written code served from the Portal's own
/// origin, so `sandbox` without `allow-same-origin` gives it an opaque origin wherever it is
/// opened, framed or in a tab of its own: it never reads the Portal's cookies or reaches into
/// a Portal window (T-2476). The flags match the run page's `<iframe sandbox>`.
fn content_security_policy(import_map: &str, basemap: Option<&str>) -> String {
    let connect = basemap.unwrap_or("'none'");
    format!(
        "default-src 'none'; base-uri 'none'; form-action 'none'; \
         script-src {} {} data:; style-src 'unsafe-inline'; img-src data: blob: {connect}; \
         font-src data:; connect-src {connect}; worker-src blob: data:; child-src blob: data:; \
         frame-ancestors 'self'; sandbox allow-scripts allow-forms",
        kit::script_hash(import_map),
        kit::script_hash(ENTRY),
    )
}

/// A module as a `data:` URL. Only what the URL parser would otherwise change is escaped: `%`
/// (the body is percent-decoded), `#` (a fragment), tabs and line breaks (removed from any
/// URL). Everything else, non-ASCII included, the parser encodes itself.
fn data_url(code: &str) -> String {
    let mut url = String::with_capacity(code.len() + code.len() / 16 + 32);
    url.push_str("data:text/javascript,");
    for c in code.chars() {
        match c {
            '%' => url.push_str("%25"),
            '#' => url.push_str("%23"),
            '\t' => url.push_str("%09"),
            '\n' => url.push_str("%0A"),
            '\r' => url.push_str("%0D"),
            c => url.push(c),
        }
    }
    url
}

/// JSON inside a `<script>` element: `<` written as `\u003c`, so nothing in it can close the element.
pub(crate) fn script_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value)
        .unwrap_or_default()
        .replace('<', "\\u003c")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> serde_json::Value {
        serde_json::json!({
            "slug": "bikes", "orgDomain": "hel.fi", "space": "mobility",
            "transport": "bridge", "appName": "bikes", "endpointName": "bikes",
        })
    }

    #[test]
    fn the_preview_runs_sandboxed_without_the_portal_origin() {
        let csp = content_security_policy("{}", None);
        let sandbox = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("sandbox"))
            .expect("a sandbox directive");
        assert_eq!(sandbox, "sandbox allow-scripts allow-forms");
        assert!(!csp.contains("allow-same-origin"), "{csp}");
        assert!(!csp.contains("allow-top-navigation"), "{csp}");
    }

    #[test]
    fn a_data_url_survives_the_url_parser() {
        let url = data_url("const a = `50%`; // #1\n\tb();\r\n");
        assert_eq!(
            url,
            "data:text/javascript,const a = `50%25`; // %231%0A%09b();%0D%0A"
        );
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.as_str(), url, "nothing is dropped or re-encoded");
    }

    #[test]
    fn nothing_in_the_data_closes_its_element() {
        let json = script_json(&serde_json::json!({ "appName": "</script><script>alert(1)" }));
        assert!(!json.contains('<'), "{json}");
    }

    #[test]
    fn the_template_files_are_embedded_without_build_output() {
        let files = template_files();
        for path in [
            MAIN,
            "src/App.tsx",
            "src/design-tokens.json",
            "functions/summary.ts",
            "package.json",
        ] {
            assert!(
                files.contains_key(path),
                "{path} missing: {:?}",
                files.keys()
            );
        }
        assert!(files
            .keys()
            .all(|p| !p.starts_with("dist/") && !p.starts_with("node_modules/")));
        assert!(
            !files.contains_key("pnpm-lock.yaml"),
            "the runner image's lockfile is not a run's context (AP-127)"
        );
    }

    #[test]
    fn a_run_whose_files_do_not_build_gets_the_problems_not_a_document() {
        let mut files = template_files();
        files.insert(
            "src/App.tsx".to_owned(),
            "import lodash from \"lodash\";\nexport default () => lodash;\n".to_owned(),
        );
        match document(&files, "bikes", &config(), None) {
            Err(Refusal::Problems(problems)) => {
                assert_eq!(problems.len(), 1, "{problems:?}");
                assert_eq!(
                    (problems[0].file.as_str(), problems[0].line),
                    ("src/App.tsx", 1)
                );
            }
            _ => panic!("a refused import built a document"),
        }
    }

    /// With the runtime built: the template is one document whose import map names every
    /// module, whose policy hashes exactly its two scripts, and which carries no session. With
    /// `JC_PREVIEW_OUT` set it is written there for the browser check in `sdk/e2e`.
    #[test]
    fn the_template_is_one_document_with_its_import_map_and_policy() {
        let files = template_files();
        let doc = match document(
            &files,
            "bikes <demo>",
            &config(),
            Some("https://portal.example/api/v1/projects/p/basemap/"),
        ) {
            Ok(doc) => doc,
            Err(Refusal::NoRuntime) => return,
            Err(Refusal::Problems(problems)) => {
                panic!("the template does not build: {problems:#?}")
            }
        };
        let html = &doc.html;
        let map_json = html
            .split("<script type=\"importmap\">")
            .nth(1)
            .and_then(|rest| rest.split("</script>").next())
            .expect("an inline import map");
        let map: serde_json::Value = serde_json::from_str(map_json).unwrap();
        for name in transpile::INTERFACE.iter().copied().chain([
            "@app/src/main.tsx",
            "@app/src/App.tsx",
            "@app/src/design-tokens.json",
        ]) {
            let url = map["imports"][name]
                .as_str()
                .unwrap_or_else(|| panic!("{name} is not in the import map"));
            assert!(url.starts_with("data:text/javascript,"), "{name}");
        }
        assert!(
            map["imports"]
                .as_object()
                .unwrap()
                .keys()
                .all(|k| !k.starts_with("@app/functions/") && !k.contains(".test.")),
            "functions and tests stay out of the interface"
        );
        assert_eq!(
            doc.csp,
            format!(
                "default-src 'none'; base-uri 'none'; form-action 'none'; script-src {} {} data:; \
                 style-src 'unsafe-inline'; img-src data: blob: https://portal.example/api/v1/projects/p/basemap/; \
                 font-src data:; connect-src https://portal.example/api/v1/projects/p/basemap/; \
                 worker-src blob: data:; child-src blob: data:; frame-ancestors 'self'; \
                 sandbox allow-scripts allow-forms",
                kit::script_hash(map_json),
                kit::script_hash(ENTRY)
            )
        );
        assert!(html.contains("<title>bikes &lt;demo&gt;</title>"));
        assert!(html
            .contains("<script id=\"jc-config\" type=\"application/json\">{\"appName\":\"bikes\""));
        assert!(html.contains("\"transport\":\"bridge\""));
        assert_eq!(html.matches("<script type=\"module\">").count(), 1);
        // The runtime's own code names the cookie its same-origin transport reads; the rest of
        // the document must name no credential at all.
        let around = html.replace(map_json, "");
        for secret in ["__Host-jc_csrf", "Bearer", "access_token", "Set-Cookie"] {
            assert!(!around.contains(secret), "{secret} in the document");
        }
        if let Ok(dir) = std::env::var("JC_PREVIEW_OUT") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(format!("{dir}/template.html"), html).unwrap();
            std::fs::write(format!("{dir}/template.csp"), &doc.csp).unwrap();
        }
    }
}
