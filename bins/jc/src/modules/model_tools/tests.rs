use super::*;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

const MODEL: &str = "\
id: https://city.example/models/parking
name: parking
default_prefix: city
default_range: string
prefixes:
  city: https://city.example/models/parking/
  linkml: https://w3id.org/linkml/
imports:
  - linkml:types
  - ngsi-ld-core
classes:
  ParkingSpot:
    is_a: Entity
    slots: [status, capacity]
slots:
  status:
    range: SpotStatus
    annotations:
      ngsi_ld_kind: Property
  capacity:
    range: integer
    annotations:
      ngsi_ld_kind: Property
enums:
  SpotStatus:
    permissible_values:
      free: {}
      occupied: {}
";

async fn call(method: &str, path: &str, body: impl Into<Body>) -> (StatusCode, Value) {
    let resp = router()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(body.into())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn source(text: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("source".to_owned(), Value::String(text.to_owned()));
    m
}

/// The one error of a model that compiles: the artifacts this port does not render.
fn unrendered() -> String {
    format!("{}: {NOT_RENDERED}", UNRENDERED.join(", "))
}

fn errors_of(v: &Value) -> Vec<String> {
    v["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn healthz_answers_and_every_other_route_is_a_json_404() {
    let (status, body) = call("GET", "/healthz", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["generatorVersion"], generator_version());
    for (method, path) in [
        ("GET", "/generate"),
        ("POST", "/infer-schema"),
        ("POST", "/import-sdm"),
        ("GET", "/catalog"),
        ("POST", "/try-mapping"),
        ("GET", "/"),
    ] {
        let (status, body) = call(method, path, Body::empty()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
        assert!(
            errors_of(&body)[0].contains("not a Model Tools route"),
            "{body}"
        );
    }
}

#[tokio::test]
async fn a_model_compiles_to_a_broker_schema_the_broker_reads_and_a_context() {
    let body = json!({"source": MODEL}).to_string();
    let (status, out) = call("POST", "/generate", body).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(errors_of(&out), vec![unrendered()]);
    let schema = &out["brokerSchema"];
    assert_eq!(schema["name"], "parking");
    assert_eq!(schema["classes"]["ParkingSpot"]["is_a"], "Entity");
    assert_eq!(schema["slots"]["capacity"]["range"], "integer");
    assert_eq!(
        schema["enums"]["SpotStatus"]["permissible_values"]["free"]["text"],
        "free"
    );
    let context = &out["context"]["@context"];
    assert_eq!(context["ParkingSpot"]["@id"], "city:ParkingSpot");
    assert_eq!(context["capacity"]["@id"], "city:capacity");
    assert_eq!(out["generatorVersion"], generator_version());
    assert!(
        out.get("jsonSchema").is_none(),
        "only the views' artifacts are filled"
    );
}

#[tokio::test]
async fn a_body_past_the_cap_is_refused_before_it_is_read_whole() {
    let big = json!({"source": format!("{MODEL}\n# {}", "x".repeat(MAX_BODY_BYTES))}).to_string();
    let (status, out) = call("POST", "/generate", big).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{out}");
    // Just under the cap is read.
    let pad = MAX_BODY_BYTES - json!({"source": MODEL}).to_string().len() - 8;
    let fits = json!({"source": format!("{MODEL}#{}", "x".repeat(pad))}).to_string();
    assert!(fits.len() <= MAX_BODY_BYTES);
    let (status, out) = call("POST", "/generate", fits).await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

#[test]
fn deep_nesting_is_refused() {
    let deep = format!(
        "id: https://x.example/m\nname: m\nclasses:\n  C:\n    description: {}x{}\n",
        "[".repeat(LIMITS.max_depth + 1),
        "]".repeat(LIMITS.max_depth + 1)
    );
    let (status, out) = generate(&source(&deep));
    assert_eq!(status, StatusCode::OK);
    assert!(out.get("brokerSchema").is_none());
    assert!(
        errors_of(&out)[0].contains("nests deeper than 32 levels"),
        "{out}"
    );
}

#[test]
fn an_alias_bomb_is_refused_while_it_expands() {
    // 1 000 nodes behind `a`, 100 of them behind `b`, 100 `b`s: ~10 million nodes from a
    // text of a few kilobytes, which serde_norway's own repetition guard lets through.
    let leaves = vec!["x"; 1000].join(", ");
    let bs = vec!["*a"; 100].join(", ");
    let cs = vec!["*b"; 100].join(", ");
    let bomb = format!(
        "id: https://x.example/m\nname: m\nannotations:\n  a: &a [{leaves}]\n  b: &b [{bs}]\n  c: [{cs}]\n"
    );
    assert!(bomb.len() < 8 * 1024);
    let started = std::time::Instant::now();
    let (status, out) = generate(&source(&bomb));
    assert_eq!(status, StatusCode::OK);
    assert!(
        errors_of(&out)[0].contains("more than 200000 YAML nodes"),
        "{out}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_url_import_is_never_fetched_and_only_known_imports_resolve() {
    for import in [
        "https://evil.example/model.yaml",
        "http://127.0.0.1:9/x",
        "file:///etc/passwd",
        "../../secrets/x",
        "schema",
        "linkml:meta",
    ] {
        let text = format!("id: https://x.example/m\nname: m\nimports:\n  - {import}\n");
        let (status, out) = generate(&source(&text));
        assert_eq!(status, StatusCode::OK, "{import}");
        assert!(out.get("brokerSchema").is_none(), "{import}");
        let errors = errors_of(&out);
        assert!(
            errors[0].contains(&format!(
                "import '{import}' is not one Model Tools resolves"
            )),
            "{import}: {errors:?}"
        );
    }
}

#[test]
fn a_platform_import_resolves_from_the_request_and_an_unresolved_one_is_a_200_with_errors() {
    let shared = "\
id: https://city.example/models/shared
name: shared
default_prefix: shared
prefixes:
  shared: https://city.example/models/shared/
imports:
  - linkml:types
  - ngsi-ld-core
slots:
  label:
    range: string
    annotations:
      ngsi_ld_kind: Property
";
    let model = MODEL.replace(
        "  - ngsi-ld-core\n",
        "  - ngsi-ld-core\n  - org.shared.v1\n",
    );
    let model = model.replace(
        "slots: [status, capacity]",
        "slots: [status, capacity, label]",
    );
    let mut body = source(&model);
    let (status, out) = generate(&body);
    assert_eq!(status, StatusCode::OK);
    assert!(out.get("brokerSchema").is_none());
    assert!(errors_of(&out)[0].starts_with("import 'org.shared.v1' is not resolved"));

    body.insert("imports".to_owned(), json!({"org.shared.v1": shared}));
    let (status, out) = generate(&body);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(errors_of(&out), vec![unrendered()], "{out}");
    assert_eq!(out["brokerSchema"]["slots"]["label"]["range"], "string");
}

#[test]
fn malformed_requests_are_refused_with_400() {
    let refused = |body: Value| {
        let Value::Object(body) = body else { panic!() };
        let (status, out) = generate(&body);
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
        assert!(!errors_of(&out).is_empty());
    };
    refused(json!({}));
    refused(json!({"source": "  "}));
    refused(json!({"source": 3}));
    refused(json!({"source": MODEL, "imports": ["org.a.v1"]}));
    refused(json!({"source": MODEL, "imports": {"https://evil.example/x": "id: x"}}));
    refused(json!({"source": MODEL, "imports": {"org.Bad.v1": "id: x"}}));
    refused(json!({"source": MODEL, "imports": {"org.a.v1": 1}}));
    let many: Map<String, Value> = (0..=MAX_IMPORTS)
        .map(|i| (format!("org.m{i}.v1"), Value::String("id: x".into())))
        .collect();
    refused(json!({"source": MODEL, "imports": many}));
}

#[tokio::test]
async fn a_non_object_body_is_a_400() {
    for body in ["[]", "not json", "\"text\""] {
        let (status, out) = call("POST", "/generate", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {out}");
    }
}

#[test]
fn duplicate_keys_and_tags_are_refused_as_python_does() {
    let (_, out) = generate(&source("id: a\nname: m\nname: n\n"));
    assert_eq!(errors_of(&out), vec!["Duplicate key: \"name\"".to_owned()]);
    for tagged in ["!local m", "!local [m]", "!local {m: 1}"] {
        let (_, out) = generate(&source(&format!(
            "id: https://x.example/m\nname: {tagged}\n"
        )));
        assert!(errors_of(&out)[0].contains("YAML tag"), "{tagged}: {out}");
    }
}

/// Parity run over a directory of LinkML files (evidence/jc-model-tools): writes each file's
/// answer beside it as `<file>.json`. `MT_PARITY_IN=<seeds> MT_PARITY_OUT=<dir> cargo test -p jc
/// --lib model_tools::tests::parity -- --ignored`.
#[test]
#[ignore = "parity harness, run by hand"]
#[allow(clippy::disallowed_methods)] // A hand-run harness reads its directories, as the database tests do.
fn parity() {
    let (Ok(input), Ok(output)) = (
        std::env::var("MT_PARITY_IN"),
        std::env::var("MT_PARITY_OUT"),
    ) else {
        return;
    };
    let mut stack = vec![std::path::PathBuf::from(&input)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.to_string_lossy().ends_with(".linkml.yaml") {
                continue;
            }
            let rel = path.strip_prefix(&input).unwrap();
            let (_, out) = generate(&source(&std::fs::read_to_string(&path).unwrap()));
            let dst = std::path::Path::new(&output).join(format!("{}.json", rel.display()));
            std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
            std::fs::write(&dst, serde_json::to_string_pretty(&out).unwrap()).unwrap();
        }
    }
}
