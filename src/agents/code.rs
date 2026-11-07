//! A code run's rules (Architecture/20 §4.1, SDK-11…SDK-14): which paths the model may write,
//! what keeps a project from being a preview, and the system prompt of the one call that
//! writes the application over the SDK template. The driver that uses them is in `oneshot`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use super::preview;
use super::transpile;

/// The row types Model Tools renders from the endpoint's model; never the model's (SDK-10).
pub const TYPES: &str = "src/jc-types.ts";

/// What frames a dashboard run's request (T-3159, AP-56): the same code on the same template as
/// an application, and only reading; the run's data needs carry no write, so a form would fail.
pub const DASHBOARD: &str = "THIS IS A DASHBOARD: a read-only application. Show the data needs as \
figures, charts and a table with filters, on one page or a few. Write no form, no edit, create \
or delete control and no call that writes: this run may only read.";

/// What frames an analysis run's request (T-3160, AP-56): a dashboard's code path, with the
/// answer to the person's question as its point. It is never published; the preview is the result.
pub const ANALYSIS: &str = "THIS IS AN ANALYSIS: one read-only page that answers the question \
below from the data needs. Lead with a short written finding in plain sentences, with the numbers \
it rests on computed from the rows, then the charts and figures that show it. Write no form, no \
edit, create or delete control and no call that writes: this run may only read.";
/// Writable files a project may hold, and their bytes together (SDK-11).
pub const MAX_FILES: usize = 80;
pub const MAX_BYTES: usize = 800_000;

/// What a refused block is told (SDK-11).
pub const REFUSAL: &str = "not a path the application may write: src/**/*.tsx, src/**/*.ts, \
     src/**/*.css, src/design-tokens.json, src/dashboard.json, functions/**/*.ts and, in a wasm App, \
     server/src/**/*.rs and migrations/*.sql; never src/main.tsx or src/jc-types.ts";

/// The server crate's manifest, which only the `wasm` template carries (T-3575).
pub const SERVER_MANIFEST: &str = "server/Cargo.toml";

/// The platform services beyond identity and data that the App can call from TypeScript, from
/// the SDK's catalog (T-3585, ADR-N-045): each one's calls, its `app.yaml` lines and quotas, for
/// the services `on` holds (T-3623). Empty while none has a call or is on, so the pack carries
/// nothing it cannot use.
pub fn services_section(catalog: &str, on: impl Fn(&str) -> bool) -> String {
    let catalog: serde_json::Value = serde_json::from_str(catalog).unwrap_or_default();
    let names = |value: &serde_json::Value, key: &str| {
        value[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("name").or(Some(item)).and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let lines: Vec<String> = catalog["services"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|service| service["always"] != true && !names(service, "typescript").is_empty())
        // A service a layer keeps off is never offered: the call would be refused (AP-164).
        .filter(|service| on(service["service"].as_str().unwrap_or_default()))
        .map(|service| {
            format!(
                "- {}: calls {}; app.yaml {}; quotas {}\n",
                service["service"].as_str().unwrap_or_default(),
                names(service, "typescript"),
                names(service, "appYaml"),
                names(service, "quotas"),
            )
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "\n## THE PLATFORM SERVICES\n\nBeyond identity and data, each needs its app.yaml lines:\n{}",
        lines.concat()
    )
}

/// The writable files that call a service `on` does not hold (AP-164, T-3623), one problem per
/// file and service: the Portal would refuse the call, so the model takes it out before the
/// version is built. The calls are the catalog's `jc().{service}.{method}`, matched as
/// `.{service}.{method}(` so `useClient().ai.complete(` counts too, and its Rust
/// `jc_app_sdk::{module}::{fn}`, matched as `{module}::{fn}(` (T-3695).
pub fn off_service_calls(
    catalog: &str,
    files: &BTreeMap<String, String>,
    on: impl Fn(&str) -> bool,
) -> Vec<String> {
    let catalog: serde_json::Value = serde_json::from_str(catalog).unwrap_or_default();
    let mut problems = Vec::new();
    for service in catalog["services"].as_array().into_iter().flatten() {
        let name = service["service"].as_str().unwrap_or_default();
        if service["always"] == true || on(name) {
            continue;
        }
        let calls: Vec<String> = service["typescript"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|call| call["name"].as_str()?.strip_prefix("jc()"))
            .chain(
                service["rust"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|call| call.as_str()?.strip_prefix("jc_app_sdk::")),
            )
            .filter_map(|call| call.split(['(', '<']).next())
            .map(|call| format!("{call}("))
            .collect();
        for (path, content) in files {
            if writable(path) && calls.iter().any(|call| content.contains(call.as_str())) {
                problems.push(format!(
                    "{path}: calls the {name} service, which this App's organization, project or \
                     App keeps off; take the call out"
                ));
            }
        }
    }
    problems
}

/// The template's service examples for one App (T-3709): a `// jc:service a b` … `// jc:end`
/// block stays while `on` holds one of its names, and the marker lines go either way. A new App
/// holds no service beyond identity and data, so its first version starts without calls
/// [`off_service_calls`] would send back, at no model call.
pub fn service_examples(text: &str, on: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut keep = true;
    for line in text.split_inclusive('\n') {
        let marker = line.trim();
        if let Some(names) = marker.strip_prefix("// jc:service ") {
            keep = names.split_whitespace().any(&on);
        } else if marker == "// jc:end" {
            keep = true;
        } else if keep {
            out.push_str(line);
        }
    }
    out
}

/// What a `wasm` run's model reads beside the SDK: the server half and its rules (AP-147).
pub const SERVER_SECTION: &str = include_str!("server_section.md");

/// The SDK as the model reads it: its API with every signature, and the names it exports.
pub const SDK_API: &str = include_str!("../../sdk/API.md");
pub const SDK_EXPORTS: &str = include_str!("../../sdk/src/sdk/index.ts");

/// Whether the model may write `path` (SDK-11).
pub fn writable(path: &str) -> bool {
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
        || path == preview::MAIN
        || path == TYPES
    {
        return false;
    }
    let under = |folder: &str, extensions: &[&str]| {
        path.starts_with(folder) && extensions.iter().any(|ext| path.ends_with(ext))
    };
    under("src/", &[".tsx", ".ts", ".css"])
        || path == "src/design-tokens.json"
        || path == "src/dashboard.json"
        || under("functions/", &[".ts"])
        || under("server/src/", &[".rs"])
        || (under("migrations/", &[".sql"]) && path.matches('/').count() == 1)
}

/// The position a person pointed at in the preview, `{file}:{line}` (SDK-46): an interface file
/// under `src/` the model may write, ending in `.tsx`, and a line from 1.
pub fn scope(src: &str) -> Option<(&str, u32)> {
    let (file, line) = src.rsplit_once(':')?;
    if line.is_empty() || !line.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let line = line.parse::<u32>().ok().filter(|line| *line >= 1)?;
    (file.starts_with("src/") && file.ends_with(".tsx") && writable(file)).then_some((file, line))
}

/// Everything that keeps `files` from being a preview, one line each with its file and line:
/// the limits of SDK-11, then every transpile error and refused import (SDK-12, SDK-14).
pub fn problems(files: &BTreeMap<String, String>) -> Vec<String> {
    let mut problems = Vec::new();
    let written: Vec<&String> = files
        .iter()
        .filter(|(path, _)| writable(path))
        .map(|(_, content)| content)
        .collect();
    if written.len() > MAX_FILES {
        problems.push(format!(
            "the application has {} writable files; at most {MAX_FILES}",
            written.len()
        ));
    }
    // Server code builds only in a `wasm` App, whose template carries the crate (T-3575).
    if !files.contains_key(SERVER_MANIFEST) {
        if let Some(path) = files
            .keys()
            .find(|path| path.starts_with("server/") || path.starts_with("migrations/"))
        {
            problems.push(format!(
                "{path}: server code and migrations belong to a wasm App, and this one is not"
            ));
        }
    }
    let bytes: usize = written.iter().map(|content| content.len()).sum();
    if bytes > MAX_BYTES {
        problems.push(format!(
            "the writable files hold {bytes} bytes; at most {MAX_BYTES}"
        ));
    }
    let mut code: BTreeMap<String, String> = files
        .iter()
        .filter(|(path, _)| path.starts_with("src/") || path.starts_with("functions/"))
        .map(|(path, content)| (path.clone(), content.clone()))
        .collect();
    // The entry is the Portal's whatever the files hold, as the preview document builds it.
    if let Some(main) = preview::Template::get(preview::MAIN) {
        code.insert(
            preview::MAIN.to_owned(),
            String::from_utf8_lossy(&main.data).into_owned(),
        );
    }
    problems.extend(
        transpile::transpile(&code)
            .problems
            .iter()
            .map(ToString::to_string),
    );
    problems
}

pub static SYSTEM: LazyLock<String> = LazyLock::new(|| {
    let names = |list: &[&str]| {
        list.iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        r#"# SYSTEM INSTRUCTION: AN APPLICATION ON THE JOINEDCONTEXT APP SDK — SEARCH/REPLACE FORMAT

You write a React 19 + TypeScript application over a template that already runs: an app shell
with pages, an overview, a page per entity type with filters, a table, a map, charts, a detail
card, a form and exports, a backend function, and a test beside each of them. The user message
holds every file of the project as it stands, the SDK's API, the row types of the endpoint, the
data needs, what the application may write, five entities per type and the request.

You answer ONCE per call; a script applies your answer mechanically. There is no tool and no
follow-up question. A run takes several calls: a small first version, then the rest of the
application, then repairs; the section THIS CALL of the user message says what this one asks for.

## WHAT TO BUILD

- Everything the request and the data call for, over the calls of the run: every page, filter, chart,
  map, table, form, export and function they need. A new page is an entry in `pages` in
  `src/App.tsx`.
- The template is scaffolding, not the design. Give this application its own: a composition
  that fits the request and the data (what opens first, a summary or a hero, which maps, charts,
  tables, cards and filters, in what order), laid out with the SDK's `Page`, `Grid`, `Card`,
  `Split`, `Sidebar` and `Tabs`, and its own titles and copy. `src/design-tokens.json` is this
  application's look, generated within the organization's branding and different from every
  other application of the project: keep it as it is unless the person asks for another look,
  and put what it does not cover in `src/app.css`. Two requests over the same endpoint
  must not look alike.
- Build with the SDK hooks and the components in `src/components/`, restyled or changed when the
  design needs it. Rewrite `src/App.tsx` and the pages freely; delete a template page, component
  or function the application does not use, together with its test.
- A later instruction may change the design as freely as the first answer did.
- Colour and a picture of the data (AP-138): the screen a person lands on shows the data in at
  least one chart in the look's palette and, when the data has a location, on a map coloured by a
  value with its legend (`EntityMap` with `color`). Never a screen of number tiles only, a plain
  list or unstyled text; replace a chart or the map with a better one, never remove the last.
- Entities are listed in the SDK's `EntityGrid` (`@joinedcontext/sdk/grid`), through the template's
  `TypeGrid`, never a hand-made table; a ratio is a column
  `{{ attr, formula: "a / b * 100" }}` (names, numbers, + - * / %). A
  dashboard's grid, filters, chart and tiles: `src/dashboard.json`, drawn by `Dashboard`, not TSX.
- Unless THIS CALL asks for a first version without tests: a test beside every page, component
  and function you add or change (`*.test.tsx`,
  `*.test.ts`), written like the template's tests: vitest, @testing-library/react,
  `stubClient` or `fakeContext` from `@joinedcontext/sdk/testing`. A page that reads data shows
  its loading state first, so the first query after `render` awaits:
  `await screen.findByRole(…)` (or `findByText`), then `getBy…` for the rest. A `getBy…` right
  after `render` reads the loading state and fails the build lane's test gate. A name or value
  the page shows in more than one place (a tile and a table row, a chart label) is queried
  inside its region, `within(screen.getByRole("table")).getByText(…)`, or with `getAllBy…`: a
  bare `getByText` of it fails with "Found multiple elements".
- `src/fixtures.ts` is what the build's browser check serves every page from, at four widths
  (T-2827): keep `ROWS` holding a few rows of every type the application reads, with the types
  and attribute names of `src/jc-types.ts` and invented values. Never copy the five entities of
  the user message into it: the repository is read by more people than the data's audience.
- A row is named on screen with `displayName(row)`, never its `id`; a value is shown with `format`,
  so a missing number reads `—`, never `NaN`. A map colours by the data's own range (`extent`).
- An attribute a row lacks is unknown, never 0, false or "": `row.x === 0`, never `!row.x`, `?? 0`,
  `|| 0` or `typeof x === "number" ? x : 0`, and a value computed from it stays `undefined`, so
  `format` and the map show it as unknown. Count such rows apart and say so on the page
  (`StatTiles` with `equals: 0` does).
- Every number, row and label comes from what the endpoint answers. What it does not serve is
  said in one line on the page, never estimated, derived from other attributes or left as sample
  text, whatever the request or the App's title asks for.
- Types and attribute names exactly as `src/jc-types.ts` and the samples spell them; import the
  row types with `import type`.
- A form or any save only when the user message says the application may write.
- Titles and labels in the language of the request.

## WHAT YOU MAY WRITE

`src/**/*.{{tsx,ts,css}}`, `src/design-tokens.json`, `src/dashboard.json` and `functions/**/*.ts`,
at most {MAX_FILES} files and {MAX_BYTES} bytes together. Never `src/main.tsx` (the Portal owns
the entry), never `src/jc-types.ts` (rendered from the endpoint's model), never `package.json`
or any configuration: a block for another path is refused.

## WHAT YOU MAY IMPORT

- Under `src/`: a relative file, {interface}.
- Under `functions/`: a relative file under `functions/`, {function}.
- In a test, additionally: {test}.
- `import type` from anywhere in the project.
Any other import is refused before a preview exists. Data goes through the SDK, never `fetch`.

## THE FORMAT RULES

1. Every block is four markers in order: the file path alone on the line right before
   `<<<<<<< SEARCH`, then `=======`, then `>>>>>>> REPLACE`. A block without its path line or
   its closing marker is not read and changes nothing.
2. To CREATE a file or REWRITE it whole, leave the SEARCH block empty. Do that for every new
   file and for every file that changes in more than a few places.
3. For a small edit, SEARCH holds at least 3 consecutive lines copied exactly from the current
   file, unique in it; REPLACE holds the new lines only.
4. Before the first block, write one or two plain sentences for the person reading the chat:
   what the application does and what you changed. After the last block, nothing. Do not wrap
   the blocks in a markdown fence.
5. If something the request asks for is out of reach (a login, a file upload, another data
   source), say so in those sentences and build what the data holds. Raw HTML or a static page
   is a page component with that markup and its own CSS file under `src/`, said in one sentence.
6. Do what the message asks and no more. A question gets its answer in the sentences, with no
   block when nothing has to change; a small request ("put a smiley on the dashboard") is a
   small edit in place, never a new page, generator or export format nobody asked for.

## THE SYNTAX

```text
src/pages/Stations.tsx
<<<<<<< SEARCH
=======
import type {{ Row }} from "@joinedcontext/sdk";
…
>>>>>>> REPLACE
src/App.tsx
<<<<<<< SEARCH
      {{ id: "overview", label: "Overview", render: () => <Overview schema={{schema}} /> }},
=======
      {{ id: "overview", label: "Overview", render: () => <Overview schema={{schema}} /> }},
      {{ id: "stations", label: "Stations", render: () => <Stations /> }},
>>>>>>> REPLACE
```
"#,
        interface = names(transpile::INTERFACE),
        function = names(transpile::FUNCTION),
        test = names(transpile::TEST),
    )
});

/// How a file goes into a model's prompt (T-3076).
#[derive(Debug, PartialEq, Eq)]
pub enum Shown {
    /// The whole text: the application's own files, and every file it changed.
    Whole,
    /// The exports of a template component the application has not changed: what it is used
    /// by, read whole on demand.
    Outline(String),
    /// Its name only: an unchanged template test, the components' stylesheet, or a file the model
    /// may not write (the README, the build and e2e set-up), which no request needs to see.
    Named,
}

/// How `path` goes into a prompt, given the template it was copied from (T-3076). Of ~200 KB
/// of template (~50k tokens) the model needs the components' exports and the files an
/// application writes: what it changed and its pages, `App.tsx`, `i18n.ts` and types stay
/// whole, an unchanged component is shown by its exports, and an unchanged test, the
/// components' stylesheet or a file it may not write by name.
pub fn shown(path: &str, content: &str, template: &BTreeMap<String, String>) -> Shown {
    if template.get(path).map(String::as_str) != Some(content) {
        return Shown::Whole;
    }
    if path.contains(".test.") || path == "src/components/components.css" || !writable(path) {
        return Shown::Named;
    }
    if path.starts_with("src/components/") && (path.ends_with(".tsx") || path.ends_with(".ts")) {
        return Shown::Outline(outline(content));
    }
    Shown::Whole
}

/// The exports of a TypeScript module: each `export` line with the comment above it, a
/// function's parameters until its body opens, and an exported interface or type whole.
pub fn outline(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut comment: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed.starts_with("/**") || trimmed.starts_with('*') || trimmed.starts_with("//") {
            comment.push(line);
            i += 1;
            continue;
        }
        if line.starts_with("export ") {
            out.append(&mut comment);
            let block = line.starts_with("export interface ")
                || (line.starts_with("export type ") && line.trim_end().ends_with('{'));
            let opens_body = |l: &str| {
                let l = l.trim_end();
                l.ends_with('{') && !l.ends_with("({") && !l.ends_with("<{")
            };
            // An interface to its closing brace; a signature until its body opens.
            let mut end = i;
            let limit = (i + 40).min(lines.len() - 1);
            if block {
                while end < limit && !lines[end].starts_with('}') {
                    end += 1;
                }
            } else if !opens_body(line) && !line.trim_end().ends_with(';') {
                while end < limit
                    && !opens_body(lines[end])
                    && !lines[end].trim_end().ends_with(';')
                {
                    end += 1;
                }
            }
            out.extend(&lines[i..=end]);
            i = end + 1;
            continue;
        }
        comment.clear();
        i += 1;
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    /// T-6442: 187 "empty" stations of 67 came from reading a missing number as 0.
    #[test]
    fn the_prompt_keeps_an_absent_attribute_unknown() {
        for rule in [
            "An attribute a row lacks is unknown, never 0",
            "never `!row.x`, `?? 0`",
            "a value computed from it stays `undefined`",
            "`StatTiles` with `equals: 0`",
        ] {
            assert!(super::SYSTEM.contains(rule), "{rule}");
        }
    }

    /// T-6442: an App over bikes, asked for air quality, printed "Average PM10 22.5" worked out
    /// of bike attributes, because the prompt said to build the nearest thing.
    #[test]
    fn the_prompt_forbids_a_value_the_endpoint_did_not_answer() {
        assert!(super::SYSTEM.contains("never estimated, derived from other attributes"));
        assert!(!super::SYSTEM.contains("nearest thing"));
    }

    /// T-3076: what of the template a prompt carries, file by file.
    #[test]
    fn an_unchanged_template_is_shown_by_its_exports_and_a_changed_file_whole() {
        let template = preview::template_files();
        let form = "src/components/EntityForm.tsx";
        let Shown::Outline(outlined) = shown(form, &template[form], &template) else {
            panic!("an unchanged component is outlined");
        };
        assert!(
            outlined.len() * 4 < template[form].len(),
            "{} of {}",
            outlined.len(),
            template[form].len()
        );
        assert!(
            outlined.contains("export function EntityForm"),
            "{outlined}"
        );
        assert_eq!(shown(form, "changed", &template), Shown::Whole);
        assert_eq!(
            shown("src/App.tsx", &template["src/App.tsx"], &template),
            Shown::Whole
        );
        assert_eq!(
            shown("src/i18n.ts", &template["src/i18n.ts"], &template),
            Shown::Whole
        );
        let test = "src/components/EntityForm.test.tsx";
        assert_eq!(shown(test, &template[test], &template), Shown::Named);
        assert_eq!(
            shown(
                "src/components/components.css",
                &template["src/components/components.css"],
                &template
            ),
            Shown::Named
        );
        assert_eq!(shown("src/pages/Bikes.tsx", "x", &template), Shown::Whole);
        assert_eq!(
            shown("README.md", &template["README.md"], &template),
            Shown::Named
        );
        // The whole template, as a prompt carries it: about a tenth of its bytes.
        let (whole, carried): (usize, usize) = template
            .iter()
            .filter(|(path, _)| writable(path))
            .map(|(path, content)| match shown(path, content, &template) {
                Shown::Whole => (content.len(), content.len()),
                Shown::Outline(text) => (content.len(), text.len()),
                Shown::Named => (content.len(), path.len()),
            })
            .fold((0, 0), |(a, b), (c, d)| (a + c, b + d));
        assert!(carried * 3 < whole, "{carried} of {whole} bytes");
    }

    #[test]
    fn an_outline_keeps_comments_signatures_and_interfaces() {
        let text = "import x from \"y\";\n/** The props. */\nexport interface Props {\n  a: string;\n}\n// hidden\nconst inner = 1;\n/** Draws it. */\nexport function Thing({\n  a,\n}: Props) {\n  return a;\n}\nexport const N = 3;\n";
        assert_eq!(
            outline(text),
            "/** The props. */\nexport interface Props {\n  a: string;\n}\n/** Draws it. */\nexport function Thing({\n  a,\n}: Props) {\nexport const N = 3;"
        );
        assert_eq!(outline(""), "");
    }

    use super::*;

    #[test]
    fn the_model_writes_interface_code_tokens_and_functions_but_not_the_entry_or_the_types() {
        for path in [
            "src/App.tsx",
            "src/pages/Stations.test.tsx",
            "src/lib/rows.ts",
            "src/app.css",
            "src/design-tokens.json",
            "src/dashboard.json",
            "functions/summary.ts",
            "functions/lib/stats.ts",
        ] {
            assert!(writable(path), "{path}");
        }
        for path in [
            "src/main.tsx",
            "src/jc-types.ts",
            "package.json",
            "index.html",
            "vite.config.ts",
            "src/data.json",
            "functions/summary.js",
            "src/../package.json",
            "src//App.tsx",
            "./src/App.tsx",
            "tests/App.tsx",
            "server/Cargo.toml",
            "server/Cargo.lock",
            "server/build.rs",
            "server/src/../Cargo.toml",
            "migrations/nested/0002.sql",
            ".gitea/workflows/build.yml",
            // A data model reaches the platform only through a Change a person proposes (T-3608).
            "bookings.linkml.yaml",
            "src/bookings.linkml.yaml",
            "projects/helsinki/spaces/bookings/datamodels/bookings.linkml.yaml",
        ] {
            assert!(!writable(path), "{path}");
        }
    }

    /// T-3585, T-3623: the pack names each service beyond identity and data the SDK has calls
    /// for, with its app.yaml lines and quotas, and never one a layer keeps off.
    #[test]
    fn the_pack_lists_the_services_that_are_on_with_their_calls() {
        let all = services_section(crate::ops::runs::SERVICES_CATALOG, |_| true);
        for line in [
            "- files: calls jc().files.put, jc().files.get, jc().files.list, jc().files.remove, jc().files.url; app.yaml services: [files], kind: wasm and storage.blob for the Rust calls; quotas filesMiB",
            "- email: calls jc().email.send; app.yaml services: [email]; quotas emailsPerDay",
            "- jobs: calls jc().jobs.list;",
            "- ai: calls jc().ai.complete; app.yaml services: [ai]; quotas aiTokensPerDay",
        ] {
            assert!(all.contains(line), "{line} in {all}");
        }
        assert!(
            !all.contains("- identity") && !all.contains("- data"),
            "{all}"
        );

        let no_ai = services_section(crate::ops::runs::SERVICES_CATALOG, |s| s != "ai");
        assert!(
            !no_ai.contains("ai.complete") && no_ai.contains("jc().email.send"),
            "{no_ai}"
        );
        assert_eq!(
            services_section(crate::ops::runs::SERVICES_CATALOG, |_| false),
            ""
        );
        let none = serde_json::json!({ "services": [
            { "service": "ai", "always": false, "typescript": [], "appYaml": ["services: [ai]"] },
        ]});
        assert_eq!(
            services_section(&none.to_string(), |_| true),
            "",
            "no call, no line"
        );
    }

    /// T-3623: a call of a service a layer keeps off is a problem of the file that makes it,
    /// whichever client it is called on; a service that is on, or a file the model does not
    /// write, is none.
    #[test]
    fn a_call_of_a_service_a_layer_keeps_off_is_a_problem() {
        let files = BTreeMap::from([
            (
                "src/Rate.tsx".to_owned(),
                "await useClient().ai.complete({ messages });".to_owned(),
            ),
            (
                "src/Mail.tsx".to_owned(),
                "await jc().email.send({ to: \"me\" });".to_owned(),
            ),
            ("src/Upload.tsx".to_owned(), "input.files.length".to_owned()),
            (
                "server/src/lib.rs".to_owned(),
                include_str!("../../sdk/template-wasm/server/src/lib.rs").to_owned(),
            ),
        ]);
        let found = off_service_calls(crate::ops::runs::SERVICES_CATALOG, &files, |s| s == "email");
        assert_eq!(
            found
                .iter()
                .map(|p| p.split(" service").next().unwrap_or_default())
                .collect::<Vec<_>>(),
            [
                "server/src/lib.rs: calls the ai",
                "src/Rate.tsx: calls the ai"
            ],
        );
        let no_mail = off_service_calls(crate::ops::runs::SERVICES_CATALOG, &files, |s| s == "ai");
        assert!(
            no_mail
                .iter()
                .any(|p| p.starts_with("server/src/lib.rs: calls the email service")),
            "{no_mail:?}"
        );
        assert!(off_service_calls(crate::ops::runs::SERVICES_CATALOG, &files, |_| true).is_empty());
        let mut unwritable = files.clone();
        unwritable.retain(|path, _| path == "src/Mail.tsx");
        unwritable.insert("package.json".to_owned(), "jc().ai.complete(".to_owned());
        assert!(
            off_service_calls(crate::ops::runs::SERVICES_CATALOG, &unwritable, |s| s
                == "email")
            .is_empty()
        );
    }

    /// T-3709: the template's service examples stay only for the services an App holds, so a
    /// new App's first version calls nothing `off_service_calls` sends back; the markers never
    /// reach an App, and every block the template opens is closed before the next one.
    #[test]
    fn the_templates_service_examples_follow_the_apps_services() {
        const LIB: &str = include_str!("../../sdk/template-wasm/server/src/lib.rs");
        let mut open = false;
        for line in LIB.lines().map(str::trim) {
            if line.starts_with("// jc:service ") {
                assert!(!open, "a block opens inside another: {line}");
                open = true;
            } else if line == "// jc:end" {
                assert!(open, "a jc:end without its block");
                open = false;
            }
        }
        assert!(!open, "the last block is never closed");
        let server = |on: &dyn Fn(&str) -> bool| {
            BTreeMap::from([("server/src/lib.rs".to_owned(), service_examples(LIB, on))])
        };
        let calls = |files: &BTreeMap<String, String>, call: &str| {
            files["server/src/lib.rs"].contains(call)
        };

        let none = server(&|_| false);
        assert!(off_service_calls(crate::ops::runs::SERVICES_CATALOG, &none, |_| false).is_empty());
        let text = &none["server/src/lib.rs"];
        for gone in [
            "// jc:",
            "fn latest(",
            "identity",
            "/api/items/digest",
            "/api/items/summary",
        ] {
            assert!(
                !text.contains(gone),
                "{gone} is left in an App without email and ai"
            );
        }
        for kept in [
            "fn checked(",
            ".post(\"/api/items\", create)",
            "router.handle(&request)",
            "jc_app_sdk::app!(handle);",
        ] {
            assert!(text.contains(kept), "{kept} went with the examples");
        }

        let both = server(&|_| true);
        assert!(calls(&both, "email::send(") && calls(&both, "ai::complete("));
        assert!(
            !both["server/src/lib.rs"].contains("// jc:service")
                && !both["server/src/lib.rs"].contains("// jc:end")
        );
        let unmarked: String = LIB
            .split_inclusive('\n')
            .filter(|line| !matches!(line.trim(), l if l.starts_with("// jc:service ") || l == "// jc:end"))
            .collect();
        assert_eq!(
            both["server/src/lib.rs"], unmarked,
            "an App with both keeps the whole template"
        );

        let mail = server(&|s| s == "email");
        assert!(calls(&mail, "email::send(") && !calls(&mail, "ai::complete("));
        assert!(
            calls(&mail, "fn latest("),
            "the digest reads the newest items"
        );
        assert!(
            off_service_calls(crate::ops::runs::SERVICES_CATALOG, &mail, |s| s == "email")
                .is_empty()
        );
    }

    /// T-3575: a `wasm` run writes its server's Rust and migrations, never the crate's manifest or
    /// lock; it starts from a template that builds, and server code in an App without the crate
    /// is named as a problem.
    #[test]
    fn a_wasm_run_writes_its_server_code_and_migrations_and_nothing_else_of_the_crate() {
        for path in [
            "server/src/lib.rs",
            "server/src/rows/mod.rs",
            "migrations/0002_bookings.sql",
        ] {
            assert!(writable(path), "{path}");
        }
        let wasm = preview::wasm_template_files();
        for path in [
            SERVER_MANIFEST,
            "server/Cargo.lock",
            "server/src/lib.rs",
            "src/server.ts",
        ] {
            assert!(wasm.contains_key(path), "{path}");
        }
        assert!(wasm.keys().all(|path| !path.contains("/target/")));
        assert_eq!(
            wasm.get(super::super::repository::WORKFLOW)
                .map(String::as_str),
            Some(super::super::repository::WASM_WORKFLOW_TEXT)
        );
        assert_eq!(problems(&wasm), Vec::<String>::new());

        let mut ui = preview::template_files();
        ui.insert("server/src/lib.rs".into(), "pub fn handle() {}".into());
        assert!(problems(&ui)
            .iter()
            .any(|problem| problem.starts_with("server/src/lib.rs: server code")));
    }

    #[test]
    fn a_scope_is_an_interface_file_and_a_line() {
        assert_eq!(
            scope("src/pages/Card.tsx:12"),
            Some(("src/pages/Card.tsx", 12))
        );
        for refused in [
            "src/pages/Card.tsx",
            "src/pages/Card.tsx:0",
            "src/pages/Card.tsx:+3",
            "src/pages/Card.tsx:",
            "src/pages/Card.ts:3",
            "src/main.tsx:1",
            "src/../x.tsx:1",
            "functions/a.tsx:1",
            "package.json:1",
            "src/pages/Card.tsx:99999999999",
        ] {
            assert_eq!(scope(refused), None, "{refused}");
        }
    }

    #[test]
    fn the_template_builds_and_what_breaks_it_is_named_with_file_and_line() {
        let mut files = preview::template_files();
        assert!(!files.is_empty(), "the template is embedded");
        assert_eq!(problems(&files), Vec::<String>::new());

        files.insert(
            "src/pages/Broken.tsx".to_owned(),
            "import axios from \"axios\";\nexport const x = axios;\n".to_owned(),
        );
        let found = problems(&files);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("src/pages/Broken.tsx:1:"), "{found:?}");
        assert!(found[0].contains("axios"), "{found:?}");
    }

    #[test]
    fn the_prompt_hands_the_design_to_the_model_and_keeps_the_rules() {
        let system = SYSTEM.as_str();
        assert!(system.contains("The template is scaffolding, not the design."));
        assert!(system.contains("`src/design-tokens.json`"));
        // T-6384: a dashboard is composed of the SDK grid, its ratios computed by formula columns.
        assert!(system.contains("`EntityGrid` (`@joinedcontext/sdk/grid`)"));
        assert!(system.contains("formula: \""));
        // T-6402: the dashboard's editable part is one file the Portal edits as a Change.
        assert!(system.contains("`src/dashboard.json`, drawn by `Dashboard`"));
        assert!(system.contains("must not look alike"));
        assert!(system.contains("Colour and a picture of the data (AP-138)"));
        assert!(system.contains("never remove the last"));
        // The look is generated per application (AP-123); the model keeps it.
        assert!(system.contains("generated within the organization's branding"));
        assert!(system.contains("Raw HTML or a static page"));
        assert!(!system.contains("delete nothing that still works"));
        // The rules that keep a project a preview are still there (SDK-11, SDK-12).
        assert!(system.contains("Never `src/main.tsx`"));
        assert!(system.contains("a test beside every page"));
        // T-2827: the browser check's rows are the model's to keep true, and invented.
        assert!(system.contains("`src/fixtures.ts`"));
        assert!(system.contains("Never copy the five entities"));
        // A page that loads shows its loading state first; a test that reads the page right
        // after `render` fails the build lane's gate (SDK-24, T-3016: alerts-desk).
        assert!(system.contains("the first query after `render` awaits"));
        assert!(system.contains("`await screen.findByRole(…)`"));
        // A value shown in a tile and a table is queried in its region: every app run on dev on
        // 2026-10-06 failed its own tests on "Found multiple elements" (T-3044).
        assert!(system.contains("`within(screen.getByRole(\"table\")).getByText(…)`"));
    }

    /// `n` writable stylesheets of `each` bytes: counted by SDK-11, and nothing a transpile reads.
    fn sheets(n: usize, each: usize) -> BTreeMap<String, String> {
        (0..n)
            .map(|i| (format!("src/s{i}.css"), " ".repeat(each)))
            .collect()
    }

    fn limit_problems(files: &BTreeMap<String, String>) -> Vec<String> {
        problems(files)
            .into_iter()
            .filter(|p| p.contains("writable files") || p.contains("bytes;"))
            .collect()
    }

    /// SDK-11, T-2510: only the paths the model may write count toward the limits.
    #[test]
    fn a_file_outside_src_and_functions_is_not_counted_toward_bytes_or_files() {
        let mut files = sheets(MAX_FILES, 1);
        files.insert("README.md".to_owned(), "a".repeat(MAX_BYTES + 1));
        files.insert("package.json".to_owned(), "{}".to_owned());
        files.insert("src/main.tsx".to_owned(), "a".repeat(10));
        assert_eq!(limit_problems(&files), Vec::<String>::new());
    }

    /// SDK-11, T-2510: the limits are inclusive, one over is a problem, for files and bytes.
    #[test]
    fn the_limits_are_inclusive_and_one_over_is_a_problem() {
        // exactly_max_files_passes, max_files_plus_one_is_a_problem
        assert_eq!(limit_problems(&sheets(MAX_FILES, 1)), Vec::<String>::new());
        let over = limit_problems(&sheets(MAX_FILES + 1, 1));
        assert_eq!(over.len(), 1, "{over:?}");
        assert!(
            over[0].contains(&format!("{} writable files", MAX_FILES + 1)),
            "{over:?}"
        );
        // exactly_max_bytes_passes, max_bytes_plus_one_is_a_problem
        assert_eq!(limit_problems(&sheets(1, MAX_BYTES)), Vec::<String>::new());
        let over = limit_problems(&sheets(1, MAX_BYTES + 1));
        assert_eq!(over.len(), 1, "{over:?}");
        assert!(
            over[0].contains(&format!("{} bytes", MAX_BYTES + 1)),
            "{over:?}"
        );
    }

    /// SDK-11, T-2510: the budget is bytes, so a text of fewer characters than the limit is still
    /// over it when its UTF-8 is.
    #[test]
    fn unicode_file_content_counts_bytes_not_chars() {
        let mut files = BTreeMap::new();
        let text = format!("/* {} */", "é".repeat(MAX_BYTES / 2));
        assert!(text.chars().count() <= MAX_BYTES && text.len() > MAX_BYTES);
        files.insert("src/app.css".to_owned(), text);
        let found = limit_problems(&files);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("bytes"), "{found:?}");
    }

    /// SDK-11, T-2510: an empty map holds no writable file, so no limit is crossed; what it lacks
    /// is the transpile's to say (the entry imports an App nothing wrote).
    #[test]
    fn an_empty_files_map_crosses_no_limit() {
        assert_eq!(limit_problems(&BTreeMap::new()), Vec::<String>::new());
    }

    /// SDK-11, SDK-12, T-2510: a limit and a refused import are both reported by the same call.
    #[test]
    fn two_problems_from_limits_and_import_both_appear_in_one_call() {
        let mut files = preview::template_files();
        files.extend(sheets(MAX_FILES + 1, 1));
        files.insert(
            "src/pages/Broken.tsx".to_owned(),
            "import axios from \"axios\";\nexport const x = axios;\n".to_owned(),
        );
        let found = problems(&files);
        assert!(
            found.iter().any(|p| p.contains("writable files")),
            "{found:?}"
        );
        assert!(found.iter().any(|p| p.contains("axios")), "{found:?}");
    }

    /// SDK-11, T-2510: an empty writable file is still a file of the project.
    #[test]
    fn a_writable_zero_byte_path_still_counts_as_one_file() {
        let mut files = sheets(MAX_FILES, 1);
        files.insert("src/empty.css".to_owned(), String::new());
        let found = limit_problems(&files);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("writable files"), "{found:?}");
    }

    #[test]
    fn a_project_over_the_limits_is_a_problem() {
        let mut files = BTreeMap::new();
        for i in 0..=MAX_FILES {
            files.insert(format!("src/x{i}.css"), "a{}".to_owned());
        }
        files.insert("src/big.css".to_owned(), "a".repeat(MAX_BYTES));
        let found = problems(&files);
        assert!(
            found.iter().any(|p| p.contains("writable files")),
            "{found:?}"
        );
        assert!(found.iter().any(|p| p.contains("bytes")), "{found:?}");
    }
}
