//! The inventory of every route the Portal serves, read from its source: the route coverage gate
//! (T-0840) and the authorization matrix (T-2797) both hold a row per route and check it here.

/// Every `.route("…", …)` under `src/`, as `(METHOD, path)` with the `/api/v1` prefix off.
///
/// ponytail: the source is the inventory because `axum`'s `Router` cannot be asked what it
/// holds. A router built inside a `#[cfg(test)]` module is a fixture, so the scan stops there.
pub fn routes_in_source() -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read the crate's sources") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read a source file");
            let text = text.split("\n#[cfg(test)]").next().unwrap_or_default();
            found.extend(routes_in(&code_of(text)));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The text without its comment lines, so a doc comment naming `.route(` is not a route.
fn code_of(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every route of `text`. A route the scan cannot read panics instead of going unlisted (T-4774):
/// a path that is not a string literal, a method other than the five below (`any`, `on`), or a
/// router mounted by `.nest_service`, `.route_service` or a `.nest` under another prefix than
/// `/api/v1`, whose routes would then pass the authorization matrix without a row.
pub fn routes_in(text: &str) -> Vec<(String, String)> {
    for mount in [".nest_service(", ".route_service("] {
        assert!(
            !text.contains(mount),
            "a router mounted by `{mount}` hides its routes from this inventory"
        );
    }
    for nest in text.split(".nest(").skip(1) {
        assert!(
            nest.trim_start().starts_with("\"/api/v1\""),
            "a router nested under another prefix than /api/v1 hides its routes from this inventory"
        );
    }
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(".route(") {
        let open = from + at + ".route(".len() - 1;
        let mut depth = 0usize;
        let mut end = open;
        for (offset, ch) in text[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        let call = &text[open..=end];
        from = end + 1;
        let Some(literal) = call[1..].trim_start().strip_prefix('"') else {
            panic!("a route whose path is not a string literal: .route{call}");
        };
        let path = &literal[..literal.find('"').expect("a closed string")];
        let path = path.strip_prefix("/api/v1").unwrap_or(path);
        let before = found.len();
        for method in ["get", "post", "put", "patch", "delete"] {
            if names_method(call, method) {
                found.push((method.to_uppercase(), path.to_owned()));
            }
        }
        assert!(
            found.len() > before,
            "a route whose method this inventory cannot read: .route{call}"
        );
    }
    found
}

/// `get(` as a method of this route, not the tail of a handler's name.
fn names_method(call: &str, method: &str) -> bool {
    let needle = format!("{method}(");
    let mut from = 0;
    while let Some(at) = call[from..].find(&needle) {
        let start = from + at;
        let before = call[..start].chars().next_back().unwrap_or(' ');
        if !before.is_alphanumeric() && before != '_' {
            return true;
        }
        from = start + needle.len();
    }
    false
}
