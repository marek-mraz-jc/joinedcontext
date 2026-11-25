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
        ("POST", "/catalog"),
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
    assert_eq!(errors_of(&out), Vec::<String>::new());
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
}

/// T-6831: every artifact the Portal's save commits (DM-02) is rendered, so a model saves on jc.
#[tokio::test]
async fn a_model_renders_every_artifact_the_save_commits() {
    let source = MODEL.replacen(
        "  linkml: https://w3id.org/linkml/\n",
        "  linkml: https://w3id.org/linkml/\n  ucefact: http://unece.org/cefact/\n",
        1,
    )
    .replacen(
        "  capacity:\n    range: integer\n",
        "  capacity:\n    range: integer\n    description: Spaces in total.\n    unit:\n      ucum_code: '1'\n      exact_mappings: [ucefact:C62]\n",
        1,
    );
    let (status, out) = call("POST", "/generate", json!({"source": source}).to_string()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(errors_of(&out), Vec::<String>::new(), "{out}");

    let spot = &out["jsonSchema"]["definitions"]["ParkingSpot"];
    assert_eq!(
        out["jsonSchema"]["$schema"],
        "http://json-schema.org/draft-07/schema#"
    );
    assert_eq!(
        out["jsonSchema"]["x-generator-version"],
        generator_version()
    );
    assert_eq!(spot["properties"]["capacity"]["type"], "integer");
    assert_eq!(
        spot["properties"]["location"]["x-ngsi-ld-kind"],
        "GeoProperty"
    );
    assert_eq!(
        spot["properties"]["status"]["enum"],
        json!(["free", "occupied"])
    );
    assert_eq!(
        spot["properties"]["capacity"]["x-unit"],
        json!({"ucumCode": "1", "exactMappings": ["ucefact:C62"]})
    );

    assert!(
        out["docs"].as_str().unwrap().contains("ParkingSpot"),
        "{}",
        out["docs"]
    );

    let example = &out["example"];
    assert_eq!(
        example["id"],
        "urn:ngsi-ld:ParkingSpot:city.example:example:1"
    );
    assert_eq!(example["type"], "ParkingSpot");
    assert_eq!(example["capacity"], 0);
    assert_eq!(example["status"], "free");
    assert_eq!(example["location"]["type"], "Point");
    let terms = out["context"]["@context"].as_object().unwrap();
    for key in example.as_object().unwrap().keys() {
        assert!(
            matches!(key.as_str(), "id" | "type") || terms.contains_key(key),
            "{key}"
        );
    }

    let ts = out["typescript"].as_str().unwrap();
    assert!(
        ts.contains("export type SpotStatus = \"free\" | \"occupied\";"),
        "{ts}"
    );
    assert!(ts.contains("export type ParkingSpot = {"), "{ts}");
    assert!(
        ts.contains("  /** Spaces in total. Unit: 1. */\n  capacity?: number | null;"),
        "{ts}"
    );
    assert!(ts.contains("  location?: Geometry | null;"), "{ts}");
    assert!(
        ts.contains("export type EntityTypeName = \"ParkingSpot\";"),
        "{ts}"
    );
}

/// A model with no entity class has no example: the save is refused naming the field.
#[tokio::test]
async fn a_model_without_an_entity_class_names_the_example_in_errors() {
    let source = MODEL.replacen("    is_a: Entity\n", "", 1);
    let (status, out) = call("POST", "/generate", json!({"source": source}).to_string()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let errors = errors_of(&out);
    assert_eq!(errors.len(), 1, "{out}");
    assert!(
        errors[0].starts_with("example: no class of this model specialises `Entity`"),
        "{out}"
    );
    assert!(
        out.get("example").is_none() && out.get("jsonSchema").is_some(),
        "{out}"
    );
}

/// A model author reads which field, prefix or name is wrong, in words, never a Python exception.
#[tokio::test]
async fn a_bad_field_prefix_or_curie_is_named_in_plain_words() {
    let cases = [
        (
            MODEL.replacen(
                "    is_a: Entity\n",
                "    is_a: Entity\n    colour: red\n",
                1,
            ),
            "'colour'",
        ),
        (
            MODEL.replacen("default_prefix: city", "default_prefix: nope", 1),
            "'nope'",
        ),
        (
            MODEL.replacen(
                "    is_a: Entity\n",
                "    is_a: Entity\n    class_uri: a:b:c\n",
                1,
            ),
            "a:b:c",
        ),
    ];
    for (source, named) in cases {
        let (_, out) = call("POST", "/generate", json!({"source": source}).to_string()).await;
        let errors = errors_of(&out).join("; ");
        assert!(errors.contains(named), "{named}: {out}");
        for python in ["KeyError", "__init__", "values to unpack"] {
            assert!(!errors.contains(python), "{named}: {out}");
        }
    }
}

/// T-6831: the catalogue is the Portal's mirror; Model Tools on jc fetches nothing, so its
/// catalogue is empty and stale, and an import names the model it does not hold.
#[tokio::test]
async fn catalog_and_import_sdm_answer_without_fetching() {
    let (status, out) = call(
        "GET",
        "/catalog?refresh=true&subject=dataModel.Parking",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        out,
        json!({"subjects": [], "refreshedAt": null, "stale": true})
    );

    let body = json!({"model": "dataModel.Environment/AirQualityObserved"}).to_string();
    let (status, out) = call("POST", "/import-sdm", body).await;
    assert_eq!(status, StatusCode::OK);
    let errors = errors_of(&out);
    assert!(
        errors[0].starts_with(
            "dataModel.Environment/AirQualityObserved is not in the Smart Data Models mirror"
        ),
        "{out}"
    );
    assert!(out.get("linkml").is_none(), "{out}");

    for bad in ["", "{}", "{\"model\": 3}", "not json"] {
        let (status, out) = call("POST", "/import-sdm", bad.to_owned()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {out}");
    }
}

/// T-6747: the broker schema keeps what the author says the model is, so a view's published
/// LinkML carries it.
#[tokio::test]
async fn the_broker_schema_keeps_the_title_description_and_license() {
    let source = MODEL.replacen(
        "name: parking\n",
        "name: parking\ntitle: Parking\ndescription: Where to park.\nlicense: https://e.org/cc-by\n",
        1,
    );
    let (status, out) = call("POST", "/generate", json!({"source": source}).to_string()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(errors_of(&out), Vec::<String>::new());
    let schema = &out["brokerSchema"];
    assert_eq!(
        (&schema["title"], &schema["description"], &schema["license"]),
        (
            &json!("Parking"),
            &json!("Where to park."),
            &json!("https://e.org/cc-by")
        )
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
    assert_eq!(errors_of(&out), Vec::<String>::new(), "{out}");
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
/// --lib model_tools::tests::parity`; without both variables it runs on a seed of its own.
#[test]
#[allow(clippy::disallowed_methods)] // A hand-run harness reads its directories, as the database tests do.
fn parity() {
    let own = std::env::temp_dir().join(format!("jc-mt-parity-{}", std::process::id()));
    let (input, output) = match (
        std::env::var("MT_PARITY_IN"),
        std::env::var("MT_PARITY_OUT"),
    ) {
        (Ok(input), Ok(output)) => (
            std::path::PathBuf::from(input),
            std::path::PathBuf::from(output),
        ),
        _ => {
            std::fs::create_dir_all(own.join("in/nested")).expect("seed dir");
            std::fs::write(
                own.join("in/nested/m.linkml.yaml"),
                "id: https://x.example/m\nname: m\n",
            )
            .expect("seed");
            std::fs::write(own.join("in/readme.txt"), "not a model").expect("other file");
            (own.join("in"), own.join("out"))
        }
    };
    let mut stack = vec![input.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.to_string_lossy().ends_with(".linkml.yaml") {
                continue;
            }
            let rel = path.strip_prefix(&input).expect("under the input");
            let (_, out) = generate(&source(&std::fs::read_to_string(&path).expect("model")));
            let dst = output.join(format!("{}.json", rel.display()));
            std::fs::create_dir_all(dst.parent().expect("parent")).expect("out dir");
            std::fs::write(&dst, serde_json::to_string_pretty(&out).expect("json")).expect("write");
        }
    }
    if own.exists() {
        let written =
            std::fs::read_to_string(own.join("out/nested/m.linkml.yaml.json")).expect("answer");
        assert!(written.starts_with('{'), "{written}");
        assert!(!own.join("out/readme.txt.json").exists());
        std::fs::remove_dir_all(&own).expect("clean");
    }
}

/// A double-quoted YAML string with an escape reaches the parser as an owned string.
#[test]
fn an_escaped_string_reads_as_text() {
    let mut used = 0;
    let v =
        schema::parse_yaml("name: \"tab\\there\"\nnone: ~\n", LIMITS, &mut used).expect("parsed");
    assert_eq!(v["name"], "tab\there");
    assert!(v["none"].is_null());
}

/// A model with every NGSI-LD kind, a type chain, units, an enum with meanings, an inline
/// object, a relationship pair, an abstract parent and a slot name that is no identifier.
const RICH: &str = r#"id: https://city.example/models/rich
name: rich
title: Rich
version: 1.2.0
description: Every kind of slot
license: CC-BY-4.0
default_prefix: city
default_range: string
prefixes:
  city: https://city.example/models/rich/
  linkml: https://w3id.org/linkml/
  ext: https://ext.example/vocab#
  qudt: https://qudt.org/vocab/unit/
  skos: http://www.w3.org/2004/02/skos/core#
emit_prefixes: [ext]
imports:
  - linkml:types
  - ngsi-ld-core
types:
  Celsius:
    typeof: Temperature
    description: degrees
  Temperature:
    typeof: float
classes:
  Place:
    is_a: Entity
    abstract: true
  Station:
    is_a: Place
    description: A measuring station
    exact_mappings: [ext:Site]
    id_prefixes: [ext]
    slots: [label, spot, level, heat, state, tags, open, opened, started, at, page, home, my-slot, note, words, extra, payload, zones]
    slot_usage:
      level:
        required: true
        annotations:
          shown: false
    annotations:
      open_world: true
      rank: 3
  Zone:
    is_a: Entity
    slots: [station]
  Address:
    attributes:
      street:
        alias: streetName
slots:
  label:
    required: true
    rank: 1
    slot_uri: ext:label
    annotations:
      ngsi_ld_kind: LanguageProperty
  spot:
    annotations:
      ngsi_ld_kind: GeoProperty
  level:
    range: integer
    unit:
      ucum_code: m
      symbol: m
      exact_mappings: [qudt:M]
    annotations:
      threshold: 4.5
      strict: true
  heat:
    range: Celsius
  state:
    range: State
  tags:
    multivalued: true
  open:
    range: boolean
  opened:
    range: date
  started:
    range: datetime
  at:
    range: time
  page:
    range: uri
  home:
    range: Address
    inlined: true
  my-slot:
    range: double
  note:
    annotations:
      ngsi_ld_kind: JsonProperty
  words:
    annotations:
      ngsi_ld_kind: VocabProperty
  extra:
    annotations:
      ngsi_ld_kind: ListProperty
  payload:
    range: decimal
    annotations:
      jsonld_keyword: "@json"
  zones:
    range: Zone
    multivalued: true
    inverse: station
    annotations:
      ngsi_ld_kind: Relationship
  station:
    range: Station
    inverse: zones
    annotations:
      ngsi_ld_kind: Relationship
enums:
  State:
    description: The state
    permissible_values:
      on:
        description: switched on
        meaning: ext:On
        annotations:
          colour: green
      off: {}
  Empty:
    permissible_values: {}
"#;

#[test]
fn a_model_with_every_kind_of_slot_renders_each_artifact() {
    let (status, out) = generate(&source(RICH));
    assert_eq!(status, StatusCode::OK);
    assert_eq!(errors_of(&out), Vec::<String>::new(), "{out}");

    let ctx = &out["context"]["@context"];
    assert_eq!(
        ctx["ext"], "https://ext.example/vocab#",
        "emit_prefixes and mappings emit ext"
    );
    assert_eq!(
        ctx["heat"]["@type"], "xsd:float",
        "Celsius → Temperature → float"
    );
    assert_eq!(
        ctx["label"],
        json!({"@id": "ext:label", "@container": "@language"})
    );
    assert_eq!(ctx["note"]["@type"], "@json");
    assert_eq!(ctx["words"]["@type"], "@vocab");
    assert_eq!(ctx["extra"]["@container"], "@list");
    assert_eq!(ctx["zones"]["@type"], "@id");
    assert_eq!(
        ctx["payload"], "@json",
        "a jsonld_keyword replaces the term"
    );
    assert_eq!(
        ctx["streetName"],
        json!({"@id": "city:street"}),
        "an attribute's alias"
    );
    assert_eq!(ctx["state"]["@context"]["meaning"], "@id");
    assert_eq!(ctx["page"]["@type"], "xsd:anyURI");

    let broker = &out["brokerSchema"];
    for (key, value) in [
        ("version", "1.2.0"),
        ("title", "Rich"),
        ("license", "CC-BY-4.0"),
        ("default_prefix", "city"),
    ] {
        assert_eq!(broker[key], value, "{key}");
    }
    let station = &broker["classes"]["Station"];
    assert_eq!(station["is_a"], "Place");
    assert_eq!(station["description"], "A measuring station");
    assert_eq!(
        station["slot_usage"]["level"],
        json!({"name": "level", "required": true, "annotations": {"shown": {"tag": "shown", "value": "false"}}}),
        "only what the usage changes"
    );
    assert_eq!(
        station["annotations"]["rank"]["value"], "3",
        "a number annotation is text"
    );
    assert_eq!(
        broker["slots"]["heat"]["range"], "float",
        "a type resolves to its built-in"
    );
    assert_eq!(broker["slots"]["label"]["rank"], 1);
    assert_eq!(
        broker["slots"]["level"]["unit"],
        json!({"ucum_code": "m", "symbol": "m", "exact_mappings": ["qudt:M"]})
    );
    assert_eq!(
        broker["slots"]["level"]["annotations"]["strict"]["value"],
        "true"
    );
    assert_eq!(
        broker["slots"]["level"]["annotations"]["threshold"]["value"],
        "4.5"
    );
    assert_eq!(
        broker["enums"]["State"],
        json!({"name": "State", "description": "The state", "permissible_values": {
            "on": {"text": "on", "description": "switched on", "meaning": "ext:On",
                   "annotations": {"colour": {"tag": "colour", "value": "green"}}},
            "off": {"text": "off"}}})
    );

    assert_eq!(
        out["jsonSchema"]["definitions"]["Station"]["properties"]["level"]["x-unit"],
        json!({"ucumCode": "m", "symbol": "m", "exactMappings": ["qudt:M"]})
    );

    // The example is of Station: Place is abstract and Zone sorts after it.
    let example = &out["example"];
    assert_eq!(example["type"], "Station");
    assert_eq!(example["id"], "urn:ngsi-ld:Station:city.example:example:1");
    for (key, value) in [
        ("label", json!({"en": "label"})),
        ("spot", json!({"type": "Point", "coordinates": [0.0, 0.0]})),
        ("level", json!(0)),
        ("heat", json!(0.0)),
        ("state", json!("off")),
        ("tags", json!(["tags"])),
        ("open", json!(true)),
        ("opened", json!("2026-01-01")),
        ("started", json!("2026-01-01T00:00:00Z")),
        ("at", json!("00:00:00")),
        ("page", json!("https://example.invalid/")),
        ("home", json!({"street": "street"})),
        ("extra", json!(["extra"])),
        ("zones", json!(["urn:ngsi-ld:Zone:city.example:example:1"])),
    ] {
        assert_eq!(example[key], value, "{key}");
    }

    let ts = out["typescript"].as_str().unwrap_or_default();
    for line in [
        "export type Empty = never;",
        "/** The state */\nexport type State = \"off\" | \"on\";",
        "  label: string;",
        "  spot?: Geometry | null;",
        "  /** Unit: m. */\n  level: number;",
        "  state?: State | null;",
        "  tags?: string | null;",
        "  open?: boolean | null;",
        "  \"my-slot\"?: number | null;",
        "  zones?: string | null;",
        "} & { [attr: string]: string | number | boolean | Geometry | null };",
        "export type EntityTypeName = \"Station\" | \"Zone\";",
    ] {
        assert!(ts.contains(line), "{line}\n{ts}");
    }
    assert!(
        !ts.contains("type: \"Place\""),
        "an abstract class has no row type"
    );
}

/// `generate` on `MODEL` with `from` replaced by `to`, its `errors`.
fn errors_with(from: &str, to: &str) -> Vec<String> {
    assert!(MODEL.contains(from), "{from}");
    let (status, out) = generate(&source(&MODEL.replacen(from, to, 1)));
    assert_eq!(status, StatusCode::OK);
    errors_of(&out)
}

#[test]
fn context_failures_name_the_term_and_hold_back_the_example() {
    let e = errors_with("  status:\n", "  status:\n    slot_uri: a:b:c\n");
    assert!(
        e[0].starts_with("context: 'a:b:c' is neither a CURIE (prefix:name, one colon) nor a URI"),
        "{e:?}"
    );
    assert!(
        e.iter()
            .any(|m| m == "example: the @context did not render"),
        "{e:?}"
    );

    let e = errors_with("  status:\n", "  status:\n    exact_mappings: [a:b:c]\n");
    assert!(
        e[0].starts_with("context: Definition status - unrecognized prefix: a:b:c"),
        "{e:?}"
    );

    let e = errors_with(
        "  ParkingSpot:\n",
        "  ParkingSpot:\n    class_uri: https://smartdatamodels.org/dataModel.Parking/ParkingSpot\n",
    );
    assert!(
        e[0].starts_with(
            "context: these terms are minted under a namespace the organisation does not own"
        ),
        "{e:?}"
    );
    assert!(e[0].ends_with("'ParkingSpot' → https://smartdatamodels.org/dataModel.Parking/ParkingSpot (reserved: https://smartdatamodels.org/)"), "{e:?}");
    // The same IRI imported with its upstream source is no squat.
    let e = errors_with(
        "  ParkingSpot:\n",
        "  ParkingSpot:\n    class_uri: https://smartdatamodels.org/dataModel.Parking/ParkingSpot\n    annotations:\n      upstream_source: https://github.com/smart-data-models/dataModel.Parking\n",
    );
    assert!(e.iter().all(|m| !m.contains("minted under")), "{e:?}");
}

#[test]
fn the_context_follows_prefixes_mappings_and_any_of() {
    let model = MODEL
        .replace("  linkml: https://w3id.org/linkml/\n", "  linkml: https://w3id.org/linkml/\n  odd: https://odd.example/x\n")
        .replace(
            "slots:\n  status:\n",
            "slots:\n  status:\n    exact_mappings: [https://nowhere.example/s, https://odd.example/xs, foreign:status]\n",
        )
        .replace("slots: [status, capacity]", "slots: [status, capacity, either, mixed, near, far]")
        .replace(
            "enums:\n",
            "  either:\n    any_of:\n      - range: ParkingSpot\n      - range: integer\n  mixed:\n    any_of:\n      - range: ParkingSpot\n      - range: integer\n      - range: date\n  near:\n    any_of:\n      - range: ParkingSpot\n    slot_uri: https://odd.example/xnear\n  far:\n    slot_uri: https://far.example/far\nenums:\n",
        );
    let (_, out) = generate(&source(&model));
    let ctx = &out["context"]["@context"];
    assert_eq!(
        ctx["odd"],
        json!({"@id": "https://odd.example/x", "@prefix": true}),
        "{out}"
    );
    assert_eq!(
        ctx["foreign"], "http://example.org/UNKNOWN/foreign/",
        "an unknown mapping prefix"
    );
    assert_eq!(
        ctx["either"]["@type"], "xsd:integer",
        "a class or one literal type"
    );
    assert!(
        ctx["mixed"].get("@type").is_none(),
        "two literal types coerce to none: {ctx}"
    );
    assert_eq!(ctx["near"]["@id"], "odd:near");
    assert_eq!(
        ctx["far"]["@id"], "https://far.example/far",
        "a URI no prefix covers stays whole"
    );
}

#[test]
fn without_a_default_prefix_the_vocab_is_the_model_id() {
    for (id, vocab) in [
        (
            "https://city.example/models/parking",
            "https://city.example/models/parking/",
        ),
        (
            "https://city.example/models/parking#",
            "https://city.example/models/parking#",
        ),
    ] {
        let model = MODEL
            .replace(
                "id: https://city.example/models/parking\n",
                &format!("id: {id}\n"),
            )
            .replace("default_prefix: city\n", "")
            .replace("default_range: string\n", "");
        let (_, out) = generate(&source(&model));
        assert_eq!(out["context"]["@context"]["@vocab"], vocab, "{out}");
    }
    let e = errors_with("default_prefix: city\n", "default_prefix: nowhere\n");
    assert!(
        e[0].contains("default_prefix 'nowhere' is not declared under prefixes"),
        "{e:?}"
    );
}

#[test]
fn a_type_the_broker_cannot_hold_is_named_and_holds_back_every_saved_artifact() {
    let model = MODEL.replace("    range: integer\n", "    range: Odd\n")
        + "types:\n  Odd:\n    uri: city:odd\n";
    let (_, out) = generate(&source(&model));
    let e = errors_of(&out);
    assert!(e[0].starts_with("brokerSchema: slot 'capacity': range 'Odd' is not a class, an enum or a type based on one of"), "{e:?}");
    assert_eq!(
        e[1],
        "jsonSchema, docs, example, typescript: the broker schema did not render"
    );
    assert!(
        out.get("context").is_some(),
        "the context renders on its own"
    );
}

#[test]
fn a_flag_the_broker_reads_otherwise_is_refused_before_a_save() {
    let model = MODEL.replace(
        "    is_a: Entity\n",
        "    is_a: Entity\n    annotations:\n      qb_dsd: \"False\"\n",
    );
    let (_, out) = generate(&source(&model));
    let e = errors_of(&out);
    assert!(
        e[0].starts_with("brokerSchema: the broker refuses this schema: "),
        "{e:?}"
    );
    assert!(out.get("brokerSchema").is_none());
}

#[test]
fn the_saved_artifacts_refuse_reserved_names_endless_nesting_and_unmapped_members() {
    let (_, out) = generate(&source(&MODEL.replace("SpotStatus", "Geometry")));
    let e = errors_of(&out);
    assert!(
        e.iter()
            .any(|m| m.starts_with("typescript: 'Geometry' is a name jc-types.ts declares itself")),
        "{e:?}"
    );

    let head = &MODEL[..MODEL.find("classes:").unwrap_or_default()];
    let nested = format!(
        "{head}classes:\n  ParkingSpot:\n    is_a: Entity\n    slots: [child]\n  Box:\n    slots: [child]\nslots:\n  child:\n    range: Box\n    inlined: true\n"
    );
    let (_, out) = generate(&source(&nested));
    let e = errors_of(&out);
    assert!(
        e.iter()
            .any(|m| m.contains("nests inline objects more than 8 deep")),
        "{e:?}"
    );

    let e = errors_with("  status:\n", "  status:\n    alias: state\n");
    assert!(
        e.iter().any(|m| m.starts_with(
            "example: the generated example carries members the generated @context does not define"
        )),
        "{e:?}"
    );
}

/// The first entry of `errors` for `text`, the whole answer when there is none.
fn first_error(text: &str) -> String {
    let (_, out) = generate(&source(text));
    out["errors"]
        .get(0)
        .and_then(Value::as_str)
        .map_or_else(|| out.to_string(), str::to_owned)
}

#[test]
fn a_document_of_the_wrong_shape_is_refused_in_plain_words() {
    let head = "id: https://x.example/m\nname: m\nimports:\n  - linkml:types\n";
    for (body, expected) in [
        (
            "classes:\n  A:\n    annotations: [a]\n",
            "annotations are a map of tag to value",
        ),
        (
            "classes: 3\n",
            "class definitions are a map of name to definition",
        ),
        ("classes:\n  - just text\n", "class entries are maps"),
        (
            "classes:\n  - is_a: Entity\n",
            "class entries name themselves",
        ),
        (
            "slots:\n  s:\n    unit: 3\n",
            "a unit is a map (ucum_code, symbol, exact_mappings, ...)",
        ),
        (
            "slots:\n  s:\n    any_of: 3\n",
            "any_of is a list of slot expressions",
        ),
        ("prefixes: [a]\n", "prefixes are a map of prefix to IRI"),
        ("classes:\n  A: 3\n", "class 'A' is not a map of metaslots"),
    ] {
        let e = first_error(&format!("{head}{body}"));
        assert!(e.contains(expected), "{body}: {e}");
    }
    assert_eq!(
        first_error("- a\n- b\n"),
        "the schema document must be a mapping of LinkML fields, not a list"
    );
    assert!(
        first_error("just text\n")
            .starts_with("the schema document must be a mapping of LinkML fields, not ")
    );
    assert!(first_error("name: m\n").contains("SchemaDefinition.id must be supplied"));
}

#[test]
fn long_forms_and_lists_read_as_linkml_reads_them() {
    let model = "\
id: https://city.example/models/long
default_prefix: long
prefixes:
  long:
    prefix_prefix: long
    prefix_reference: https://city.example/models/long/
  linkml: https://w3id.org/linkml/
imports:
  - linkml:types
  - ngsi-ld-core
subsets:
  core:
    description: the core terms
classes:
  - name: Meter
    is_a: Entity
    slots: [reading, empty, counted]
    annotations:
      shown:
        tag: shown
        value: yes
  - name: Spare
slots:
  reading:
    range: 3
  empty:
    unit:
  counted:
    range: integer
    inlined_as_list: true
    any_of:
      - range: integer
types:
  '3':
    typeof: integer
";
    let (_, out) = generate(&source(model));
    assert_eq!(errors_of(&out), Vec::<String>::new(), "{out}");
    let broker = &out["brokerSchema"];
    assert_eq!(
        broker["name"], "long",
        "the name falls back to the id's last segment"
    );
    assert_eq!(
        broker["slots"]["reading"]["range"], "integer",
        "a number range is its text"
    );
    assert!(
        broker["slots"]["empty"].get("unit").is_none(),
        "a null unit is none"
    );
    assert_eq!(
        broker["classes"]["Meter"]["slot_usage"]["counted"]["inlined"], true,
        "inlined_as_list inlines: {broker}"
    );
    assert_eq!(
        broker["classes"]["Meter"]["annotations"]["shown"]["value"],
        "yes"
    );
    assert_eq!(
        broker["classes"]["Spare"]["name"], "Spare",
        "a class with no metaslots"
    );
    assert_eq!(
        out["context"]["@context"]["long"],
        "https://city.example/models/long/"
    );
}

#[test]
fn a_name_the_model_does_not_define_is_named() {
    for (from, to, expected) in [
        ("    is_a: Entity\n", "    is_a: Nowhere\n", "Nowhere"),
        (
            "    slots: [status, capacity]\n",
            "    slots: [status, capacity, gone]\n",
            "No such slot gone",
        ),
        ("  capacity:\n", "  capacity:\n    is_a: absent\n", "absent"),
    ] {
        let e = errors_with(from, to);
        assert!(e.iter().any(|m| m.contains(expected)), "{to}: {e:?}");
    }
}

#[test]
fn types_resolve_through_typeof_and_their_uri_coerces_the_term() {
    let model = MODEL.replace("slots: [status, capacity]", "slots: [status, capacity, link, plain, odd, chained]")
        .replace("enums:\n", "  link:\n    range: Link\n  plain:\n    range: Bare\n  odd:\n    range: Strange\n  chained:\n    range: Child\nenums:\n")
        + "types:\n  Link:\n    typeof: uriorcurie\n    uri: http://www.w3.org/ns/shex#iri\n  Bare:\n    typeof: string\n    uri: xsd:string\n  Strange:\n    typeof: string\n    uri: odd:thing\n  Child:\n    typeof: Parent\n  Parent:\n    typeof: integer\n";
    let (_, out) = generate(&source(&model));
    let e = errors_of(&out);
    assert!(
        e[0].starts_with("context: Unknown CURIE prefix: odd"),
        "{e:?}"
    );
    let fixed = model.replace("    uri: odd:thing\n", "    uri: thing\n");
    let (_, out) = generate(&source(&fixed));
    assert!(
        errors_of(&out)[0].starts_with("context: Unknown CURIE prefix: @base (thing)"),
        "{out}"
    );
    let fixed = model.replace("    uri: odd:thing\n", "    uri: xsd:token\n");
    let (_, out) = generate(&source(&fixed));
    assert_eq!(errors_of(&out), Vec::<String>::new(), "{out}");
    let ctx = &out["context"]["@context"];
    // A shex:iri type coerces to `@id`, which only a Relationship keeps: a Property drops it.
    assert_eq!(ctx["link"], json!({"@id": "city:link"}), "{ctx}");
    assert!(
        ctx["plain"].get("@type").is_none(),
        "xsd:string coerces to nothing"
    );
    assert_eq!(
        ctx["chained"]["@type"], "xsd:integer",
        "Child → Parent → integer"
    );
    assert_eq!(out["brokerSchema"]["slots"]["chained"]["range"], "integer");
}

#[test]
fn a_member_name_is_quoted_in_jc_types_only_when_it_is_no_identifier() {
    let model = MODEL
        .replace(
            "slots: [status, capacity]",
            "slots: [status, capacity, _hidden, $ref, a_b$c, 9lives, counts]",
        )
        .replace(
            "enums:\n",
            "  _hidden: {}\n  $ref: {}\n  a_b$c: {}\n  9lives: {}\n  counts:\n    range: integer\n    multivalued: true\nenums:\n",
        );
    let (_, out) = generate(&source(&model));
    let ts = out["typescript"].as_str().unwrap_or_default();
    for line in [
        "  _hidden?: string | null;",
        "  $ref?: string | null;",
        "  a_b$c?: string | null;",
        "  \"9lives\"?: string | null;",
        "  counts?: string | null;",
    ] {
        assert!(ts.contains(line), "{line}: {out}");
    }
}

#[test]
fn an_inline_object_may_nest_eight_levels_and_not_nine() {
    // L0 (the entity) holds L1 inline, ..., each level one deeper; the deepest is plain.
    let chain = |levels: usize| {
        let head = &MODEL[..MODEL.find("classes:").unwrap_or_default()];
        let mut text = format!("{head}classes:\n  L0:\n    is_a: Entity\n    slots: [next0]\n");
        for i in 1..=levels {
            text += &format!("  L{i}:\n    slots: [next{i}]\n");
        }
        text += "slots:\n";
        for i in 0..levels {
            text += &format!("  next{i}:\n    range: L{}\n    inlined: true\n", i + 1);
        }
        text += &format!("  next{levels}:\n    range: string\n");
        errors_of(&generate(&source(&text)).1)
    };
    assert_eq!(
        chain(8),
        Vec::<String>::new(),
        "eight levels of inline objects"
    );
    let e = chain(9);
    assert!(
        e.iter()
            .any(|m| m.starts_with("example: `L9` nests inline objects more than 8 deep")),
        "{e:?}"
    );
}

#[test]
fn a_whole_number_stays_a_number_in_the_broker_schema_and_a_fraction_is_text() {
    let compiled = |rank: &str| {
        let model = MODEL.replace("  capacity:\n", &format!("  capacity:\n    rank: {rank}\n"));
        generate(&source(&model)).1
    };
    for (rank, expected) in [("-1", json!(-1)), ("7", json!(7))] {
        let out = compiled(rank);
        assert_eq!(
            out["brokerSchema"]["slots"]["capacity"]["rank"], expected,
            "{rank}: {out}"
        );
    }
    // A fraction goes as its text, which the broker then refuses as no whole number.
    let e = errors_of(&compiled("1.5"));
    assert!(
        e[0].ends_with("invalid type: string \"1.5\", expected i64"),
        "{e:?}"
    );
}

#[test]
fn a_default_prefix_written_as_the_model_iri_is_refused_before_a_save() {
    let e = errors_with(
        "default_prefix: city\n",
        "default_prefix: https://city.example/models/parking/\n",
    );
    assert!(
        e.iter().any(|m| m.contains("brokerSchema: the broker refuses this schema: malformed LinkML schema: default_prefix \"https://city.example/models/parking/\" is not declared")),
        "{e:?}"
    );
}

#[tokio::test]
async fn spawn_serves_on_a_loopback_port_until_shutdown() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let shutdown = CancellationToken::new();
    let tasks = TaskTracker::new();
    let addr = spawn(shutdown.clone(), &tasks).await.expect("bound");
    assert!(addr.ip().is_loopback(), "{addr}");
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nhost: model-tools\r\nconnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await.expect("read");
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.contains("\"status\":\"ok\""), "{reply}");
    shutdown.cancel();
    tasks.close();
    tokio::time::timeout(Duration::from_secs(5), tasks.wait())
        .await
        .expect("the listener stops on shutdown");
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn an_empty_body_asks_for_the_source() {
    let (status, out) = call("POST", "/generate", Body::empty()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        errors_of(&out),
        vec!["'source' must be the LinkML document as a string".to_owned()]
    );
}

async fn body_of(resp: Response) -> (StatusCode, Value) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_stopping_module_and_a_slow_compilation_answer_503() {
    let body = || Body::from(json!({"source": MODEL}).to_string());
    let closed = Arc::new(Semaphore::new(1));
    closed.close();
    let state = State {
        slots: closed,
        timeout: COMPILE_TIMEOUT,
    };
    let (status, out) = body_of(generate_route(state, body()).await).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(errors_of(&out), vec!["model tools is stopping".to_owned()]);

    // 150 000 YAML nodes take the parser milliseconds, a nanosecond's timeout none.
    let big = format!(
        "id: https://x.example/m\nname: m\nannotations:\n  a: [{}]\n",
        vec!["x"; 150_000].join(", ")
    );
    let slots = Arc::new(Semaphore::new(1));
    let state = State {
        slots: slots.clone(),
        timeout: Duration::from_nanos(1),
    };
    let big = Body::from(json!({ "source": big }).to_string());
    let (status, out) = body_of(generate_route(state, big).await).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        errors_of(&out),
        vec!["the generator did not answer within 0 s".to_owned()]
    );
    // The compilation past its timeout keeps its slot until it ends.
    let permit = tokio::time::timeout(Duration::from_secs(10), slots.acquire_owned()).await;
    assert!(permit.is_ok(), "the slot comes back when the job ends");
}

#[test]
fn imported_definitions_never_replace_the_model_s_own() {
    let shared = "\
id: https://city.example/models/shared
name: shared
default_prefix: shared
prefixes:
  shared: https://city.example/models/shared/
  city: https://elsewhere.example/city/
  extra: https://extra.example/
imports:
  - linkml:types
  - org.shared.v1
classes:
  ParkingSpot:
    description: the import's own
slots:
  capacity:
    range: string
enums:
  SpotStatus:
    permissible_values:
      broken: {}
";
    let model = MODEL.replace(
        "  - ngsi-ld-core\n",
        "  - ngsi-ld-core\n  - org.shared.v1\n",
    );
    let mut body = source(&model);
    body.insert("imports".to_owned(), json!({ "org.shared.v1": shared }));
    let (_, out) = generate(&body);
    assert_eq!(errors_of(&out), Vec::<String>::new(), "{out}");
    let broker = &out["brokerSchema"];
    assert!(
        broker["classes"]["ParkingSpot"]
            .get("description")
            .is_none(),
        "{broker}"
    );
    assert_eq!(broker["slots"]["capacity"]["range"], "integer");
    assert!(
        broker["enums"]["SpotStatus"]["permissible_values"]
            .get("broken")
            .is_none()
    );
    let ctx = &out["context"]["@context"];
    assert_eq!(
        ctx["extra"], "https://extra.example/",
        "a prefix only the import declares"
    );
}

#[test]
fn usages_attributes_and_units_reach_each_artifact() {
    let model = MODEL
        .replace(
            "    slots: [status, capacity]\n",
            "    slots: [status, capacity, ref, code]\n    slot_usage:\n      ref:\n        identifier: true\n      code:\n        required: true\n    attributes:\n      capacity: {}\n",
        )
        .replace(
            "enums:\n",
            "  ref:\n    alias: reference\n  code:\n    any_of:\n      - description: no range\n      - range: ParkingSpot\n    unit:\n      symbol: pcs\nenums:\n",
        )
        .replacen("classes:\n", "classes:\n  Garage:\n    is_a: Entity\n    attributes:\n      capacity:\n        range: integer\n", 1);
    // A usage that makes a slot the identifier: the broker refuses it beside Entity's id.
    let (_, out) = generate(&source(&model));
    assert!(
        errors_of(&out)[0].contains("a second identifier \"ref\""),
        "{out}"
    );
    let model = model.replace(
        "      ref:\n        identifier: true\n",
        "      ref:\n        required: true\n",
    );
    let (_, out) = generate(&source(&model));
    assert_eq!(errors_of(&out), Vec::<String>::new(), "{out}");
    let ctx = &out["context"]["@context"];
    assert_eq!(
        ctx["reference"],
        json!({"@id": "city:ref"}),
        "the alias names the term"
    );
    // any_of with a class and no literal type coerces to `@id`, which a Property drops.
    assert_eq!(ctx["code"], json!({"@id": "city:code"}));
    assert_eq!(
        out["jsonSchema"]["definitions"]["ParkingSpot"]["properties"]["code"]["x-unit"],
        json!({"symbol": "pcs", "exactMappings": []})
    );
    let e = errors_with("  status:\n", "  status:\n    slot_uri: :status\n");
    assert!(
        e[0].contains(
            "\":status\" is not an IRI, a CURIE on a declared prefix or an IRI-safe name"
        ),
        "{e:?}"
    );
}

#[test]
fn a_type_with_neither_uri_nor_parent_or_a_type_cycle_is_named() {
    let base = MODEL.replace("    range: integer\n", "    range: Loose\n");
    let e = errors_of(
        &generate(&source(
            &(base.clone() + "types:\n  Loose:\n    description: no uri, no typeof\n"),
        ))
        .1,
    );
    assert!(
        e[0].starts_with("context: range 'Loose' names no type of the schema or its imports"),
        "{e:?}"
    );
    let e = errors_of(
        &generate(&source(
            &(base.clone() + "types:\n  Loose:\n    typeof: Other\n  Other:\n    typeof: Loose\n"),
        ))
        .1,
    );
    assert!(
        e.iter().any(|m| m.contains("Loose")),
        "a cycle is refused: {e:?}"
    );
    let e = errors_of(&generate(&source(&(base + "types:\n  Loose:\n    typeof: Ghost\n"))).1);
    assert!(
        e.iter().any(|m| m.starts_with(
            "brokerSchema: slot 'capacity': range 'Loose' is not a class, an enum or a type based on one of"
        )),
        "a chain that ends on no built-in: {e:?}"
    );
}

#[test]
fn more_document_shapes_are_refused_in_plain_words() {
    assert_eq!(
        first_error("true\n"),
        "the schema document must be a mapping of LinkML fields, not a boolean"
    );
    assert_eq!(
        first_error("3\n"),
        "the schema document must be a mapping of LinkML fields, not a number"
    );
    assert!(
        first_error("~\n")
            .starts_with("the schema document must be a mapping of LinkML fields, not ")
    );
    let head = "id: https://x.example/m\nname: m\nimports:\n  - linkml:types\n";
    for (body, expected) in [
        (
            "prefixes:\n  p:\n    prefix_prefix: p\n",
            "Prefix.prefix_reference must be supplied",
        ),
        (
            "classes:\n  A:\n    annotations:\n      a:\n        tag: a\n        value: 1\n        annotations: [x]\n",
            "annotations are a map of tag to value",
        ),
    ] {
        let e = first_error(&format!("{head}{body}"));
        assert!(e.contains(expected), "{body}: {e}");
    }
    // An any_of item that is no map carries no range and is skipped.
    assert!(!first_error(&format!("{head}slots:\n  s:\n    any_of: [3]\n")).contains("any_of"));
    // A null class is a class with no metaslots, and an import of itself is read once.
    let e = first_error(&format!("{head}classes:\n  Spare:\n"));
    assert!(!e.contains("Spare"), "{e}");
}
