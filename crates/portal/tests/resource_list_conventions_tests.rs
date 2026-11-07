//! T-5965: every kind's list and get follow API/13 (filter, cursor paging, ETag and If-Match,
//! problem+json), and a list never counts or pages over a manifest the caller may not read.
//! One loop over every kind the resource API serves, so a new kind is checked the day it lands.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{self, KindInfo};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Reads, proposes and deletes every kind in the project.
fn reader() -> Identity {
    common::doors::member("rita", "all-readers")
}

/// Reads every kind, in space `air` alone.
fn air_reader() -> Identity {
    common::doors::member("ana", "air-readers")
}

/// The project a kind's manifests live in for this test: its home seen from `ovzdusie`.
fn home(info: &KindInfo) -> String {
    resource::home(info, common::doors::PROJECT)
}

fn world() -> AppState {
    let state = common::doors::state();
    let kinds: Vec<&str> = resource::kinds().map(|info| info.kind).collect();
    state.mirror.upsert(common::envelope(
        "Role",
        "all-kinds",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read", "propose", "delete"] }] }),
    ));
    let mut bindings = vec![(
        "air-readers",
        "air-readers",
        json!({ "contextSpace": "air" }),
    )];
    for project in [common::doors::PROJECT, ORG_NAMESPACE] {
        bindings.push(("all-readers", "all-readers", json!({ "project": project })));
    }
    for (index, (_, group, scope)) in bindings.into_iter().enumerate() {
        state.mirror.upsert(common::envelope(
            "RoleBinding",
            &format!("conventions-{index}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "group": group }], "role": "all-kinds", "scope": scope }),
        ));
    }
    // A space-bound binding names spaces that exist in the project.
    for space in ["air", "water"] {
        state.mirror.upsert(common::envelope(
            "ContextSpace",
            space,
            common::doors::PROJECT,
            json!({}),
        ));
    }
    for info in resource::kinds() {
        for (name, team, space) in [
            ("t5965-a", "air", "air"),
            ("t5965-b", "air", "air"),
            ("t5965-c", "water", "air"),
            ("t5965-d", "air", "water"),
        ] {
            let mut manifest = common::envelope(
                info.kind,
                name,
                &home(info),
                json!({ "contextSpaceRef": space }),
            );
            manifest.metadata.labels.insert("team".into(), team.into());
            state.mirror.upsert(manifest);
        }
    }
    state
}

struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

async fn send(
    state: &AppState,
    who: Identity,
    method: &str,
    uri: &str,
    extra: &[(header::HeaderName, &str)],
) -> Reply {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, common::cookie(&state.config, who))
        .header(joinedcontext_portal::auth::csrf::CSRF_HEADER, common::CSRF)
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in extra {
        request = request.header(name, *value);
    }
    let body = if method == "GET" || method == "DELETE" {
        Body::empty()
    } else {
        Body::from(json!({}).to_string())
    };
    let response = server::app(state.clone())
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    Reply {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

fn names(reply: &Reply) -> Vec<String> {
    reply.body["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.pointer("/metadata/name").and_then(Value::as_str))
        .filter(|name| name.starts_with("t5965-"))
        .map(str::to_owned)
        .collect()
}

fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

fn problem(reply: &Reply, status: StatusCode, slug: &str, what: &str) {
    assert_eq!(reply.status, status, "{what}: {}", reply.body);
    assert_eq!(
        reply.headers[header::CONTENT_TYPE],
        "application/problem+json",
        "{what}"
    );
    assert_eq!(
        reply.body["type"],
        format!("https://joinedcontext.com/errors/{slug}"),
        "{what}"
    );
}

#[tokio::test]
async fn every_kind_filters_pages_and_refuses_as_the_conventions_say() {
    let state = world();
    let mut checked = 0;
    for info in resource::kinds() {
        let base = format!("/api/v1/projects/{}/{}", home(info), info.plural);
        let kind = info.kind;
        let air = encode(r#"labels.team = "air""#);

        // Filter, then page one by one through the cursor to past the end.
        let mut seen = Vec::new();
        let mut uri = format!("{base}?filter={air}&limit=1");
        for _ in 0..5 {
            let page = send(&state, reader(), "GET", &uri, &[]).await;
            assert_eq!(page.status, StatusCode::OK, "{kind} {uri}: {}", page.body);
            seen.extend(names(&page));
            match page.body["nextCursor"].as_str() {
                Some(cursor) => {
                    uri = format!("{base}?filter={air}&limit=1&cursor={}", encode(cursor))
                }
                None => break,
            }
        }
        assert_eq!(seen, ["t5965-a", "t5965-b", "t5965-d"], "{kind}: pages");

        let all = send(&state, reader(), "GET", &format!("{base}?limit=500"), &[]).await;
        assert!(
            all.body.get("nextCursor").is_none(),
            "{kind}: one page has no next"
        );
        let past = send(
            &state,
            reader(),
            "GET",
            &format!("{base}?limit=1&continue=zzz"),
            &[],
        )
        .await;
        assert_eq!(names(&past), Vec::<String>::new(), "{kind}: past the end");

        let order = encode("labels.team desc");
        let ordered = send(
            &state,
            reader(),
            "GET",
            &format!("{base}?orderBy={order}"),
            &[],
        )
        .await;
        assert_eq!(
            names(&ordered),
            ["t5965-c", "t5965-a", "t5965-b", "t5965-d"],
            "{kind}: orderBy"
        );

        // A cursor carries its filter: replayed under another one it is refused.
        let first = send(
            &state,
            reader(),
            "GET",
            &format!("{base}?filter={air}&limit=1"),
            &[],
        )
        .await;
        let cursor = encode(first.body["nextCursor"].as_str().expect("a cursor"));
        let water = encode("labels.team = water");
        let replayed = send(
            &state,
            reader(),
            "GET",
            &format!("{base}?filter={water}&cursor={cursor}"),
            &[],
        )
        .await;
        problem(&replayed, StatusCode::BAD_REQUEST, "invalid-request", kind);
        let other = send(
            &state,
            air_reader(),
            "GET",
            &format!("{base}?filter={air}&cursor={cursor}"),
            &[],
        )
        .await;
        assert_ne!(
            other.status,
            StatusCode::OK,
            "{kind}: another caller's cursor"
        );

        let unknown = send(
            &state,
            reader(),
            "GET",
            &format!(
                "{base}?filter={}",
                encode("labels.team = air AND nosuch.field = 1")
            ),
            &[],
        )
        .await;
        problem(&unknown, StatusCode::BAD_REQUEST, "invalid-request", kind);
        let detail = unknown.body["detail"].as_str().unwrap_or_default();
        assert!(
            detail.contains("nosuch.field") && detail.contains("position 22"),
            "{kind}: {detail}"
        );

        // ETag on a read, 304 on an unchanged one, 412 on a stale write.
        let one = send(&state, reader(), "GET", &format!("{base}/t5965-a"), &[]).await;
        assert_eq!(one.status, StatusCode::OK, "{kind}: {}", one.body);
        let etag = one.headers[header::ETAG].to_str().expect("etag").to_owned();
        assert!(etag.starts_with('"') && etag.len() == 66, "{kind}: {etag}");
        let again = send(
            &state,
            reader(),
            "GET",
            &format!("{base}/t5965-a"),
            &[(header::IF_NONE_MATCH, &etag)],
        )
        .await;
        assert_eq!(again.status, StatusCode::NOT_MODIFIED, "{kind}");
        let other_etag = send(&state, reader(), "GET", &format!("{base}/t5965-b"), &[]).await;
        assert_ne!(other_etag.headers[header::ETAG], etag.as_str(), "{kind}");
        for method in ["PUT", "PATCH", "DELETE"] {
            let stale = send(
                &state,
                reader(),
                method,
                &format!("{base}/t5965-a?dryRun=All"),
                &[(header::IF_MATCH, "\"stale\"")],
            )
            .await;
            problem(
                &stale,
                StatusCode::PRECONDITION_FAILED,
                "precondition-failed",
                kind,
            );
            assert_eq!(
                stale.headers[header::ETAG],
                etag.as_str(),
                "{kind} {method}"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, resource::kinds().count());
}

/// API/13 section 2: the read check runs before the filter and the page, so a manifest of
/// another space never shifts a page, shows in remainingItemCount or answers 412.
#[tokio::test]
async fn every_kind_counts_and_pages_over_readable_manifests_alone() {
    let state = world();
    let mut checked = Vec::new();
    for info in resource::kinds().filter(|info| info.kind != "ContextSpace") {
        let base = format!("/api/v1/projects/{}/{}", home(info), info.plural);
        let kind = info.kind;
        let page = send(&state, air_reader(), "GET", &format!("{base}?limit=1"), &[]).await;
        if page.status == StatusCode::NOT_FOUND {
            // A kind no space-bound grant reads at all is not there for this caller (R20).
            continue;
        }
        assert_eq!(page.status, StatusCode::OK, "{kind}: {}", page.body);
        assert_eq!(names(&page), ["t5965-a"], "{kind}");
        assert_eq!(
            page.body["metadata"]["remainingItemCount"], 2,
            "{kind}: t5965-d of space water is not counted"
        );
        let air = encode(r#"labels.team = "air""#);
        let mut seen = Vec::new();
        let mut uri = format!("{base}?filter={air}&limit=1");
        for _ in 0..5 {
            let page = send(&state, air_reader(), "GET", &uri, &[]).await;
            seen.extend(names(&page));
            match page.body["nextCursor"].as_str() {
                Some(cursor) => {
                    uri = format!("{base}?filter={air}&limit=1&cursor={}", encode(cursor))
                }
                None => break,
            }
        }
        assert_eq!(seen, ["t5965-a", "t5965-b"], "{kind}: filter over readable");
        let hidden = send(
            &state,
            air_reader(),
            "PUT",
            &format!("{base}/t5965-d?dryRun=All"),
            &[(header::IF_MATCH, "\"stale\"")],
        )
        .await;
        assert_ne!(
            hidden.status,
            StatusCode::PRECONDITION_FAILED,
            "{kind}: a 412 would tell that t5965-d exists"
        );
        checked.push(kind);
    }
    eprintln!(
        "space-bound reads checked for {} kinds: {checked:?}",
        checked.len()
    );
    // The kinds that live in a context space (spec.contextSpaceRef); a space-bound grant reads no
    // other kind, which answers 404 above.
    assert!(
        checked.len() >= 8,
        "only {checked:?} were read by a space-bound grant"
    );
}

/// The MCP tool takes the REST list's filter, with the same refusal (API/13 section 8).
#[tokio::test]
async fn jc_resource_list_takes_the_same_filter_for_every_kind() {
    let state = world();
    let caller = common::doors::session(reader());
    for info in resource::kinds() {
        let kind = info.kind;
        let listed = common::doors::call(
            "jc_resource_list",
            &caller,
            &state,
            json!({ "kind": kind, "filter": "labels.team = air" }),
        )
        .await;
        assert_eq!(listed.status, StatusCode::OK, "{kind}: {}", listed.text());
        let mut names: Vec<&str> = listed.body["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row["name"].as_str())
            .filter(|name| name.starts_with("t5965-"))
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["t5965-a", "t5965-b", "t5965-d"], "{kind}");
        let refused = common::doors::call(
            "jc_resource_list",
            &caller,
            &state,
            json!({ "kind": kind, "filter": "nosuch.field = 1" }),
        )
        .await;
        assert_eq!(
            refused.status,
            StatusCode::BAD_REQUEST,
            "{kind}: {}",
            refused.text()
        );
    }
}

/// T-6175, API/13 section 2: the one list contract on every kind: limit, sort, q, count and a
/// cursor that pages under its own sort and search.
#[tokio::test]
async fn every_list_takes_the_one_list_contract() {
    let state = world();
    let errors = |reply: &Reply, field: &str| {
        reply.body["errors"]
            .as_array()
            .is_some_and(|all| all.iter().any(|e| e["path"] == field))
    };
    // The organization's Endpoints list is checked in edge_resource_reads_tests, beside its
    // administrator.
    for info in resource::kinds() {
        let base = &format!("/api/v1/projects/{}/{}", home(info), info.plural);
        let who = reader;
        for (query, field) in [
            ("limit=0", "limit"),
            ("limit=1001", "limit"),
            ("limit=many", "limit"),
            ("count=yes", "count"),
            ("sort=a%20b", "sort"),
            ("sort=name&orderBy=name", "orderBy"),
            ("cursor=forged.x", "cursor"),
        ] {
            let bad = send(&state, who(), "GET", &format!("{base}?{query}"), &[]).await;
            problem(&bad, StatusCode::BAD_REQUEST, "invalid-request", base);
            assert!(errors(&bad, field), "{base}?{query}: {}", bad.body);
        }
        let unknown = send(
            &state,
            who(),
            "GET",
            &format!("{base}?sort=-nosuch.field"),
            &[],
        )
        .await;
        problem(
            &unknown,
            StatusCode::UNPROCESSABLE_ENTITY,
            "unprocessable",
            base,
        );
        assert!(errors(&unknown, "sort"), "{base}: {}", unknown.body);

        // Descending by name, searched for "t5965", two rows a page: the cursor walks it whole.
        let start = format!("{base}?sort=-metadata.name&q=T5965&limit=2&count=true");
        let mut uri = start.clone();
        let mut seen = Vec::new();
        for _ in 0..5 {
            let page = send(&state, who(), "GET", &uri, &[]).await;
            assert_eq!(page.status, StatusCode::OK, "{uri}: {}", page.body);
            assert_eq!(
                page.body["totalCount"], 4,
                "{uri}: every page counts the whole listing"
            );
            seen.extend(names(&page));
            match page.body["nextCursor"].as_str() {
                Some(cursor) => uri = format!("{start}&cursor={}", encode(cursor)),
                None => break,
            }
        }
        assert_eq!(seen, ["t5965-d", "t5965-c", "t5965-b", "t5965-a"], "{base}");
        let plain = send(&state, who(), "GET", &format!("{base}?limit=1"), &[]).await;
        assert!(
            plain.body.get("totalCount").is_none(),
            "{base}: only with count=true"
        );
        let cursor = encode(plain.body["nextCursor"].as_str().expect("a cursor"));
        let other_q = send(
            &state,
            who(),
            "GET",
            &format!("{base}?limit=1&q=x&cursor={cursor}"),
            &[],
        )
        .await;
        problem(&other_q, StatusCode::BAD_REQUEST, "invalid-request", base);
        let none = send(
            &state,
            who(),
            "GET",
            &format!("{base}?q=no-such-text&count=true"),
            &[],
        )
        .await;
        assert_eq!(none.body["totalCount"], 0, "{base}");
    }
}
