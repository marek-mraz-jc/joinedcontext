//! The command line's own answers: each verb's output and exit code on success, on a refusal and
//! on a malformed flag, run through the built binary the way an operator or a Job runs it.

use crate::common::{self, demo_repo, temp_dir, write};
use std::path::Path;
use std::process::{Command, Output};

fn jcctl(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jcctl"))
        .args(args)
        .env_remove("JC_GATEWAY_URL")
        .env_remove("JC_TOKEN_FILE")
        .env_remove("JC_SERVER")
        .env_remove("JC_MODEL_TOOLS_URL")
        .output()
        .expect("jcctl runs")
}

fn path(dir: &Path) -> &str {
    dir.to_str().expect("a UTF-8 temp path")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn is_usage(out: &Output) -> bool {
    out.status.code() == Some(1) && stderr(out).starts_with("usage:")
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.org"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// An executable shell script standing in for the Portal binary.
#[cfg(unix)]
fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let file = dir.join(name);
    std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).expect("script");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    file
}

/// `validate --as-portal` hands the arguments to the Portal binary and exits with its code: 0,
/// its own non-zero code, 1 when a signal ended it, and 2 naming JC_PORTAL_BIN when it does not
/// start.
#[cfg(unix)]
#[test]
fn validate_as_portal_passes_on_the_portals_exit_code() {
    let dir = temp_dir("cli-as-portal");
    let run = |bin: &Path| {
        Command::new(env!("CARGO_BIN_EXE_jcctl"))
            .args(["validate", "--as-portal", "--repo-dir", "r"])
            .env("JC_PORTAL_BIN", bin)
            .output()
            .expect("jcctl runs")
    };
    let args = dir.join("args");
    let ok = script(
        &dir,
        "ok",
        &format!("echo \"$@\" > {}; exit 0", args.display()),
    );
    assert_eq!(run(&ok).status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(&args).expect("the args").trim(),
        "validate --as-portal --repo-dir r"
    );
    assert_eq!(run(&script(&dir, "three", "exit 3")).status.code(), Some(3));
    assert_eq!(
        run(&script(&dir, "killed", "kill -9 $$")).status.code(),
        Some(1)
    );
    let missing = run(&dir.join("no-such-portal"));
    assert_eq!(missing.status.code(), Some(2));
    assert!(stderr(&missing).contains("JC_PORTAL_BIN"), "{missing:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `roles seed` writes the seeded roles once and says so; the second run writes nothing; a
/// repository path that is a file is an error. `roles render` lists what it wrote.
#[test]
fn roles_seed_and_render_report_what_they_wrote() {
    let dir = demo_repo("cli-roles");
    let first = jcctl(&["roles", "seed", "--repo-dir", path(&dir)]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    assert!(stdout(&first).contains("wrote "), "{first:?}");
    let second = jcctl(&["roles", "seed", "--repo-dir", path(&dir)]);
    assert_eq!(second.status.code(), Some(0));
    assert!(stdout(&second).contains("nothing written"), "{second:?}");
    let file = dir.join("org.yaml");
    let broken = jcctl(&["roles", "seed", "--repo-dir", path(&file)]);
    assert_eq!(broken.status.code(), Some(1));
    assert!(stderr(&broken).starts_with("error: "), "{broken:?}");

    let rendered = jcctl(&["roles", "render", "--repo-dir", path(&dir)]);
    assert_eq!(rendered.status.code(), Some(0), "{rendered:?}");
    assert!(!rendered.stdout.is_empty(), "{rendered:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `roles input` prints the gate input as JSON with the author, the e-mail and the groups; a
/// missing listing names its path, a path out of the checkout is refused, and an empty
/// e-mail or an unknown flag is handled before anything is read.
#[test]
fn roles_input_prints_the_gate_input() {
    let dir = demo_repo("cli-roles-input");
    let changes = dir.join("changes.txt");
    std::fs::write(&changes, "M\tprojects/ovzdusie/project.yaml\n").expect("listing");
    let run = |changes: &Path, email: &str| {
        jcctl(&[
            "roles",
            "input",
            "--repo-dir",
            path(&dir),
            "--base-dir",
            path(&dir),
            "--changes",
            path(changes),
            "--author",
            "jana",
            "--author-email",
            email,
            "--groups",
            "editors,,admins",
        ])
    };
    let out = run(&changes, "jana@example.org");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let input: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(input["author"], "jana");
    assert_eq!(input["groups"], serde_json::json!(["editors", "admins"]));
    let no_email = run(&changes, "");
    let input: serde_json::Value = serde_json::from_slice(&no_email.stdout).expect("JSON");
    assert!(input["author_email"].is_null(), "{input}");

    let missing = run(&dir.join("absent.txt"), "j@example.org");
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("absent.txt"), "{missing:?}");
    let outside = dir.join("outside.txt");
    std::fs::write(&outside, "M\t../../etc/passwd\n").expect("listing");
    assert_eq!(run(&outside, "j@example.org").status.code(), Some(1));
    assert!(is_usage(&jcctl(&["roles", "input", "--bogus", "x"])));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `fmt` names a file it cannot format and exits 1.
#[test]
fn fmt_names_a_file_it_cannot_read_as_yaml() {
    let dir = temp_dir("cli-fmt");
    write(&dir, "broken.yaml", "kind: [unclosed\n");
    let out = jcctl(&["fmt", path(&dir.join("broken.yaml"))]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("broken.yaml"), "{out:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The client verbs read the Portal address from JC_SERVER when --server is absent, and a
/// Portal that does not answer is an error, not a hang; `ops` without a project is usage.
#[test]
fn the_client_takes_its_server_from_the_environment() {
    let dir = temp_dir("cli-server");
    let token = dir.join("token");
    std::fs::write(&token, "a-token").expect("token");
    let out = Command::new(env!("CARGO_BIN_EXE_jcctl"))
        .args(["get", "endpoints", "--project", "ovzdusie", "--token-file"])
        .arg(&token)
        .env("JC_SERVER", "http://127.0.0.1:9")
        .output()
        .expect("jcctl runs");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(stderr(&out).contains("127.0.0.1:9"), "{out:?}");
    assert!(is_usage(&jcctl(&["ops"])));
    assert!(is_usage(&jcctl(&["plan", "--bogus"])));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `artifacts rebuild` takes --space and --revision, refuses a key without `=` or a trailing
/// flag, and on success counts what it wrote.
#[test]
fn artifacts_rebuild_counts_what_it_wrote() {
    let dir = demo_repo("cli-artifacts");
    let out_dir = temp_dir("cli-artifacts-out");
    let out = jcctl(&[
        "artifacts",
        "rebuild",
        "--repo-dir",
        path(&dir),
        "--out-dir",
        path(&out_dir),
        "--project-key",
        "ovzdusie=bbbbbbbbbbbbbbbbbbbbbbbbbb/cccccccccccccccccccccccccc",
        "--space",
        "ovzdusie",
        "--revision",
        "abc1234",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(stdout(&out).contains("objects written to"), "{out:?}");
    assert!(is_usage(&jcctl(&[
        "artifacts",
        "rebuild",
        "--repo-dir",
        "r",
        "--out-dir",
        "o",
        "--project-key",
        "no-equals",
    ])));
    assert!(is_usage(&jcctl(&[
        "artifacts",
        "rebuild",
        "--repo-dir",
        "r",
        "--out-dir",
        "o",
        "--space",
    ])));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// `artifacts rebuild` names, on stderr, each declared file the repository does not hold and
/// points at the command that makes it.
#[test]
fn artifacts_rebuild_names_a_declared_file_that_is_missing() {
    let dir = demo_repo("cli-artifacts-missing");
    write(
        &dir,
        "projects/ovzdusie/spaces/ovzdusie/datamodels/air.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: DataModel\nmetadata:\n  name: air\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: ovzdusie\n  linkml: ./air.linkml.yaml\n  version: 1.0.0\n  lifecycle: published\n  classes: [Air]\n",
    );
    let out_dir = temp_dir("cli-artifacts-missing-out");
    let out = jcctl(&[
        "artifacts",
        "rebuild",
        "--repo-dir",
        path(&dir),
        "--out-dir",
        path(&out_dir),
        "--project-key",
        "ovzdusie=bbbbbbbbbbbbbbbbbbbbbbbbbb/cccccccccccccccccccccccccc",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(
        stderr(&out).contains("air/linkml is declared and not in the repository"),
        "{out:?}"
    );
    assert!(stdout(&out).contains("0 objects written to"), "{out:?}");

    let unkeyed = jcctl(&[
        "artifacts",
        "rebuild",
        "--repo-dir",
        path(&dir),
        "--out-dir",
        path(&out_dir),
    ]);
    assert_eq!(unkeyed.status.code(), Some(1), "{unkeyed:?}");
    assert!(stderr(&unkeyed).contains("--project-key"), "{unkeyed:?}");
}

/// `plan`, `apply` and `drift` read --project-dir as `<slug>=<path>`; a bare value is usage and
/// an organization path that does not read is an error before anything is planned.
#[test]
fn a_project_dir_without_a_slug_is_usage_and_a_missing_org_an_error() {
    let dir = demo_repo("cli-project-dir");
    let bare = jcctl(&["plan", "--repo-dir", path(&dir), "--project-dir", "nowhere"]);
    assert!(is_usage(&bare), "{bare:?}");
    let file = dir.join("org.yaml");
    let missing = jcctl(&["plan", "--repo-dir", path(&file)]);
    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `plan` with a gateway: a token file that does not read is an error naming it; `drift`
/// prints the seed report as text or JSON against a gateway that does not answer as an error.
#[test]
fn plan_and_drift_with_a_gateway_that_is_not_there() {
    let dir = demo_repo("cli-plan-gateway");
    let token = dir.join("token");
    std::fs::write(&token, "a-token").expect("token");
    let unreadable = jcctl(&[
        "plan",
        "--repo-dir",
        path(&dir),
        "--gateway-url",
        "http://127.0.0.1:9",
        "--token-file",
        path(&dir.join("absent")),
    ]);
    assert_eq!(unreadable.status.code(), Some(1), "{unreadable:?}");
    let drift_unreadable = jcctl(&[
        "drift",
        "--repo-dir",
        path(&dir),
        "--gateway-url",
        "http://127.0.0.1:9",
        "--token-file",
        path(&dir.join("absent")),
    ]);
    assert_eq!(drift_unreadable.status.code(), Some(1));
    let bad_url = jcctl(&[
        "plan",
        "--repo-dir",
        path(&dir),
        "--gateway-url",
        "not a url",
        "--token-file",
        path(&token),
    ]);
    assert_eq!(bad_url.status.code(), Some(1), "{bad_url:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A repository that declares a seed entity and a gateway that does not answer: `plan` and
/// `drift` stop with the read that failed, and `apply` (on the reviewed main) stops before it
/// writes anything.
#[test]
fn a_seed_the_gateway_cannot_be_asked_about_stops_plan_drift_and_apply() {
    let dir = demo_repo("cli-seed-gateway");
    write(
        &dir,
        "projects/ovzdusie/spaces/ovzdusie/entities/seed/station.json",
        r#"{"id":"urn:ngsi-ld:Station:bb.sk:ovzdusie:1","type":"Station"}"#,
    );
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "reviewed"]);
    git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let token = temp_dir("cli-seed-gateway-token").join("token");
    std::fs::write(&token, "a-token").expect("token");
    for verb in ["plan", "drift", "apply"] {
        let out = jcctl(&[
            verb,
            "--repo-dir",
            path(&dir),
            "--gateway-url",
            "http://127.0.0.1:9",
            "--token-file",
            path(&token),
        ]);
        assert_eq!(out.status.code(), Some(1), "{verb}: {out:?}");
        assert!(
            stderr(&out).contains("http://127.0.0.1:9"),
            "{verb} names the gateway: {out:?}"
        );
    }
}

/// `validate --project` takes --slug and --org-domain alongside --param.
#[test]
fn validate_project_takes_slug_and_org_domain() {
    let dir = temp_dir("cli-validate-project");
    let out = jcctl(&[
        "validate",
        "--project",
        path(&dir),
        "--slug",
        "ovzdusie",
        "--org-domain",
        "bb.sk",
    ]);
    assert!(!is_usage(&out), "{out:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `changelog` prints the page of a project checkout's history, and a path that is no
/// checkout is an error.
#[test]
fn changelog_prints_the_page_or_the_error() {
    let repo = demo_repo("cli-changelog");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "the project"]);
    let out = jcctl(&["changelog", "--repo-dir", path(&repo), "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(!out.stdout.is_empty());
    let plain = temp_dir("cli-changelog-plain");
    let refused = jcctl(&["changelog", "--repo-dir", path(&plain.join("absent"))]);
    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&plain);
}

const POLICY: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Policy\nmetadata:\n  name: public-air\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: { kind: ContextSpace, name: ovzdusie }\n  assigner: did:web:banskabystrica.sk\n  assignee: { kind: role, id: public }\n  operations: [queryEntity]\n  information:\n    - entities:\n        - type: AirQualityObserved\n";

/// `policy export` then `policy import` through the command: the round trip prints the same
/// manifest, an unreadable file is named, and import's flags are checked.
#[test]
fn policy_export_and_import_through_the_command() {
    let dir = temp_dir("cli-policy");
    write(&dir, "policy.yaml", POLICY);
    let set = jcctl(&["policy", "export", path(&dir.join("policy.yaml"))]);
    assert_eq!(set.status.code(), Some(0), "{set:?}");
    std::fs::write(dir.join("set.jsonld"), &set.stdout).expect("set");
    let back = jcctl(&["policy", "import", path(&dir.join("set.jsonld"))]);
    assert_eq!(back.status.code(), Some(0), "{back:?}");
    assert_eq!(stdout(&back), POLICY);
    let missing = jcctl(&["policy", "export", path(&dir.join("absent.yaml"))]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("absent.yaml"), "{missing:?}");
    assert!(is_usage(&jcctl(&["policy", "import", "--name"])));
    assert!(is_usage(&jcctl(&["policy"])));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Policy helpers name the file for every failure: YAML that is no Policy, a Policy that
/// does not validate, a set that is no JSON, and an --out it cannot create.
#[test]
fn policy_failures_name_the_file() {
    use jcctl::commands::policy::{export, import, ImportOptions};
    let dir = temp_dir("cli-policy-fail");
    write(&dir, "not-policy.yaml", "kind: [unclosed\n");
    let refused = export(&dir.join("not-policy.yaml")).expect_err("no Policy");
    assert!(refused.contains("not-policy.yaml"), "{refused}");
    write(
        &dir,
        "invalid.yaml",
        &POLICY.replace("operations: [queryEntity]", "operations: []"),
    );
    let refused = export(&dir.join("invalid.yaml")).expect_err("no operation");
    assert!(refused.contains("invalid.yaml"), "{refused}");
    write(&dir, "set.jsonld", "{ not json");
    let options = ImportOptions::parse(&[path(&dir.join("set.jsonld"))]).expect("parses");
    let refused = import(&options).expect_err("not JSON");
    assert!(refused.contains("set.jsonld"), "{refused}");
    write(&dir, "policy.yaml", POLICY);
    std::fs::write(
        dir.join("good.jsonld"),
        export(&dir.join("policy.yaml")).expect("exports"),
    )
    .expect("set");
    let out = dir.join("no-dir").join("out.yaml");
    let options = ImportOptions::parse(&[path(&dir.join("good.jsonld")), "--out", path(&out)])
        .expect("parses");
    let refused = import(&options).expect_err("no parent directory");
    assert!(refused.contains("out.yaml"), "{refused}");
    let options = ImportOptions::parse(&[
        path(&dir.join("good.jsonld")),
        "--out",
        path(&dir.join("written.yaml")),
    ])
    .expect("parses");
    assert!(import(&options).expect("writes").starts_with("wrote "));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A Policy ODRL has no form for (a per-attribute capability) is refused on export, naming the file.
#[test]
fn policy_export_refuses_what_odrl_cannot_say() {
    use jcctl::commands::policy::export;
    let dir = temp_dir("cli-policy-no-odrl");
    write(
        &dir,
        "attributes.yaml",
        &POLICY.replace(
            "assignee: { kind: role, id: public }",
            "assignee: { kind: organization }",
        ),
    );
    let refused = export(&dir.join("attributes.yaml")).expect_err("no ODRL form");
    assert!(refused.contains("attributes.yaml"), "{refused}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `migrate mqtt-roles` on a repository without a role list rewrites nothing and exits 0.
#[test]
fn migrate_mqtt_roles_over_nothing_to_move() {
    let dir = demo_repo("cli-mqtt-roles");
    let out = jcctl(&["migrate", "mqtt-roles", "--repo-dir", path(&dir)]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `migrate mqtt-roles` names each file it rewrote, and a role list it had to leave (an account
/// no manifest declares) on stderr with exit 1, the rest still moved.
#[test]
fn migrate_mqtt_roles_reports_what_it_moved_and_what_it_left() {
    let dir = temp_dir("cli-mqtt-roles-moved");
    write(
        &dir,
        "projects/air/access/serviceaccounts/fleet.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ServiceAccount\nmetadata:\n  name: fleet\n  namespace: air\nspec:\n  owner:\n    user: demo.steward\n  purpose: sensors\n  roles:\n    - role: space-writer\n      scope:\n        project: air\n  credentials:\n    - kind: oauth-client\n      name: default\n",
    );
    let broker = |accounts: &str| {
        format!("apiVersion: joinedcontext.com/v1alpha1\nkind: MqttBroker\nmetadata:\n  name: air\n  namespace: air\nspec:\n  roles:\n    - name: sensor\n      serviceAccounts: [{accounts}]\n      publish: [\"air/${{clientid}}/pm10\"]\n")
    };
    write(&dir, "projects/air/mqttbrokers/air.yaml", &broker("fleet"));
    let moved = jcctl(&["migrate", "mqtt-roles", "--repo-dir", path(&dir)]);
    assert_eq!(moved.status.code(), Some(0), "{moved:?}");
    assert!(stdout(&moved).contains("rewrote projects/air/access/serviceaccounts/fleet.yaml"));
    assert!(stdout(&moved).contains("rewrote projects/air/mqttbrokers/air.yaml"));

    write(&dir, "projects/air/mqttbrokers/air.yaml", &broker("ghost"));
    let left = jcctl(&["migrate", "mqtt-roles", "--repo-dir", path(&dir)]);
    assert_eq!(left.status.code(), Some(1), "{left:?}");
    assert!(stderr(&left).contains("left: "), "{left:?}");
    assert!(stderr(&left).contains("ghost"), "{left:?}");
}

/// The Model Tools verbs need a URL: without --url or JC_MODEL_TOOLS_URL each says so; `infer`
/// names a sample it cannot read and a Model Tools that does not answer.
#[test]
fn model_verbs_need_a_url_and_a_readable_sample() {
    let dir = demo_repo("cli-model");
    let no_url = jcctl(&["model", "validate", "--repo-dir", path(&dir)]);
    assert_eq!(no_url.status.code(), Some(1));
    assert!(stderr(&no_url).contains("--url"), "{no_url:?}");
    let import = jcctl(&[
        "model",
        "import",
        "dataModel.Environment/AirQualityObserved",
        "--out",
        "x",
    ]);
    assert_eq!(import.status.code(), Some(1));
    assert!(stderr(&import).contains("--url"), "{import:?}");
    let missing = jcctl(&[
        "model",
        "infer",
        "--file",
        path(&dir.join("absent.csv")),
        "--url",
        "http://127.0.0.1:9",
    ]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("cannot read"), "{missing:?}");
    write(&dir, "sample.csv", "id,pm10\ns1,12\n");
    let dead = jcctl(&[
        "model",
        "infer",
        "--file",
        path(&dir.join("sample.csv")),
        "--url",
        "http://127.0.0.1:9",
    ]);
    assert_eq!(dead.status.code(), Some(1), "{dead:?}");
    for partial in [
        &["model", "import", "id", "--url", "u"][..],
        &["model", "import", "id", "--out", "o", "--url"],
        &["model", "infer", "--url", "u"],
        &["model", "infer", "--file", "f", "--url"],
        &["model", "generate", "--repo-dir", "r", "--url"],
    ] {
        assert!(is_usage(&jcctl(partial)), "{partial:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `publish ckan`: the flags parse, a repository that does not load is an error, and a project
/// whose Endpoints publish nothing says so.
#[test]
fn publish_ckan_options_and_a_project_with_nothing_to_publish() {
    let dir = demo_repo("cli-publish");
    write(
        &dir,
        common::ENDPOINT_PATH,
        &common::ENDPOINT.replace("audience: public", "audience: organization"),
    );
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "reviewed"]);
    git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let out = jcctl(&[
        "publish",
        "ckan",
        "--repo-dir",
        path(&dir),
        "--project",
        "ovzdusie",
        "--host",
        "127.0.0.1:9",
        "--organization-title",
        "Mesto",
        "--docs-url",
        "https://docs.example.org",
        "--api-token-env",
        "JCCTL_CLI_UNSET_TOKEN",
        "--age-key-file",
        "/nonexistent/age.key",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(stdout(&out).contains("catalogue dataset"), "{out:?}");
    let empty = temp_dir("cli-publish-empty");
    let broken = jcctl(&[
        "publish",
        "ckan",
        "--repo-dir",
        path(&empty.join("absent")),
        "--project",
        "ovzdusie",
        "--host",
        "h",
    ]);
    assert_eq!(broken.status.code(), Some(1), "{broken:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `publish ckan` over an Endpoint naming a CkanInstance the project does not declare prints why
/// it is not a dataset and fails; one naming another project's instance stops the walk.
#[test]
fn publish_ckan_says_why_an_endpoint_is_not_a_dataset() {
    let committed = |test: &str, publish: &str| {
        let dir = demo_repo(test);
        write(
            &dir,
            common::ENDPOINT_PATH,
            &format!("{}  publish:\n    ckan:\n{publish}", common::ENDPOINT),
        );
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "reviewed"]);
        git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        dir
    };
    let publish = |dir: &Path| {
        jcctl(&[
            "publish",
            "ckan",
            "--repo-dir",
            path(dir),
            "--project",
            "ovzdusie",
            "--host",
            "127.0.0.1:9",
        ])
    };
    let missing = committed(
        "cli-publish-no-instance",
        "      instanceRef: { kind: CkanInstance, name: open-data }\n",
    );
    let out = publish(&missing);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr(&out).contains(
            "ovzdusie/public-air: publish.ckan.instanceRef names CkanInstance/ovzdusie/open-data"
        ),
        "{out:?}"
    );

    let foreign = committed(
        "cli-publish-foreign-instance",
        "      instanceRef: { kind: CkanInstance, name: open-data, namespace: doprava }\n",
    );
    let out = publish(&foreign);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
}

/// `pipeline test` of a pipeline whose compute step Bento does not run fails naming the kind.
#[test]
fn pipeline_test_refuses_a_compute_step_bento_does_not_run() {
    let dir = temp_dir("cli-pipeline-wasm");
    write(
        &dir,
        "pipeline.yaml",
        &format!(
            "{}  compute: {{ kind: wasm, module: ./compute, function: run }}\n",
            PIPELINE
        ),
    );
    write(&dir, "sample.json", "[{\"id\": \"s1\"}]");
    let out = jcctl(&[
        "pipeline",
        "test",
        "--pipeline",
        path(&dir.join("pipeline.yaml")),
        "--sample",
        path(&dir.join("sample.json")),
    ]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(stderr(&out).to_lowercase().contains("wasm"), "{out:?}");
}

/// `export` of a project whose tree links out of the checkout is refused, not copied.
#[cfg(unix)]
#[test]
fn export_refuses_a_project_tree_that_links_out_of_the_checkout() {
    let repo = demo_repo("cli-export-link");
    let outside = temp_dir("cli-export-link-outside");
    std::fs::write(outside.join("secret.txt"), "outside").expect("a host file");
    std::os::unix::fs::symlink(
        outside.join("secret.txt"),
        repo.join("projects/ovzdusie/notes.txt"),
    )
    .expect("a link out");
    let out_dir = temp_dir("cli-export-link-out");
    let out = jcctl(&[
        "export",
        "--repo-dir",
        path(&repo),
        "--project",
        "ovzdusie",
        "--out-dir",
        path(&out_dir),
    ]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(!out_dir.join("projects/ovzdusie/notes.txt").exists());
}

/// `export` writes the project's files and names each redacted credential on stderr; a project
/// the checkout does not hold is an error. `import` of that bundle as text lists each resource.
#[test]
fn export_names_redactions_and_import_lists_resources() {
    let repo = demo_repo("cli-export");
    write(
        &repo,
        "projects/ovzdusie/datasources/mqtt.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: DataSource\nmetadata:\n  name: mqtt-mesto\n  namespace: ovzdusie\nspec:\n  kind: mqtt\n  mqtt:\n    urls: [\"mqtts://mqtt.mesto.sk:8883\"]\n    topics: [\"air/#\"]\n  password: \"hunter2\"\n",
    );
    let out_dir = temp_dir("cli-export-out");
    let out = jcctl(&[
        "export",
        "--repo-dir",
        path(&repo),
        "--project",
        "ovzdusie",
        "--out-dir",
        path(&out_dir),
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(stdout(&out).contains("files written to"), "{out:?}");
    assert!(stderr(&out).contains("redacted DataSource/mqtt-mesto: password"));
    assert!(!stderr(&out).contains("hunter2"));
    let into_file = jcctl(&[
        "export",
        "--repo-dir",
        path(&repo),
        "--project",
        "ovzdusie",
        "--out-dir",
        path(&repo.join("org.yaml")),
    ]);
    assert_eq!(into_file.status.code(), Some(1), "{into_file:?}");

    let dest = demo_repo("cli-import-dest");
    std::fs::remove_dir_all(dest.join("projects/ovzdusie/spaces")).expect("empty project");
    let imported = jcctl(&[
        "import",
        path(&out_dir),
        "--repo-dir",
        path(&dest),
        "--conflict",
        "replace",
    ]);
    assert_eq!(imported.status.code(), Some(0), "{imported:?}");
    assert!(
        stdout(&imported).contains("manifests written to"),
        "{imported:?}"
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&out_dir);
    let _ = std::fs::remove_dir_all(&dest);
}

const PIPELINE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: air-csv\n  namespace: ovzdusie\nspec:\n  class: resident\n  source:\n    dataSourceRef: { kind: DataSource, name: shmu-csv }\n  targetEndpoint: urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all\n";

/// `pipeline test` prints the harness for a sample, its format from --format or the extension;
/// an unreadable manifest, one that is no Pipeline, one that does not validate and an
/// unreadable sample are errors naming the file.
#[test]
fn pipeline_test_prints_the_harness_or_names_the_file() {
    let dir = temp_dir("cli-pipeline-test");
    write(&dir, "pipeline.yaml", PIPELINE);
    write(&dir, "sample.json", "[{\"id\": \"s1\"}]");
    write(&dir, "sample.txt", "s1,12\n");
    let run = |pipeline: &str, sample: &str, format: &[&str]| {
        let mut args = vec![
            "pipeline".to_owned(),
            "test".to_owned(),
            "--pipeline".to_owned(),
            path(&dir.join(pipeline)).to_owned(),
            "--sample".to_owned(),
            path(&dir.join(sample)).to_owned(),
        ];
        args.extend(format.iter().map(|s| (*s).to_owned()));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        jcctl(&args)
    };
    let json = run("pipeline.yaml", "sample.json", &[]);
    assert_eq!(json.status.code(), Some(0), "{json:?}");
    let harness: serde_json::Value = serde_json::from_slice(&json.stdout).expect("JSON");
    assert!(harness["pipeline"].is_object(), "{harness}");
    for format in ["csv", "text"] {
        let out = run("pipeline.yaml", "sample.txt", &["--format", format]);
        assert_eq!(out.status.code(), Some(0), "{format}: {out:?}");
    }
    let missing = run("absent.yaml", "sample.json", &[]);
    assert!(stderr(&missing).contains("absent.yaml"), "{missing:?}");
    write(&dir, "other.yaml", common::ORG);
    let other = run("other.yaml", "sample.json", &[]);
    assert_eq!(other.status.code(), Some(1));
    assert!(stderr(&other).contains("other.yaml"), "{other:?}");
    write(
        &dir,
        "invalid.yaml",
        &PIPELINE.replace("name: air-csv", "name: Not_A_Name"),
    );
    let invalid = run("invalid.yaml", "sample.json", &[]);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(stderr(&invalid).contains("invalid.yaml"), "{invalid:?}");
    let no_sample = run("pipeline.yaml", "absent.json", &[]);
    assert_eq!(no_sample.status.code(), Some(1));
    assert!(stderr(&no_sample).contains("absent.json"), "{no_sample:?}");
    assert!(is_usage(&run(
        "pipeline.yaml",
        "sample.json",
        &["--format", "xml"]
    )));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Without `JC_PORTAL_BIN` the Portal binary is looked up by its name; without `USER` an export
/// names `jcctl` as who took it.
#[test]
fn the_portal_binary_and_the_exporter_have_defaults() {
    let out = Command::new(env!("CARGO_BIN_EXE_jcctl"))
        .args(["validate", "--as-portal", "--repo-dir", "r"])
        .env_remove("JC_PORTAL_BIN")
        .env("PATH", temp_dir("cli-no-portal"))
        .output()
        .expect("jcctl runs");
    assert!(!out.status.success(), "{out:?}");

    let repo = demo_repo("cli-export-no-user");
    let out_dir = temp_dir("cli-export-no-user-out");
    let export = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_jcctl"))
            .args(args)
            .env_remove("USER")
            .output()
            .expect("jcctl runs")
    };
    let out = export(&[
        "export",
        "--repo-dir",
        path(&repo),
        "--project",
        "ovzdusie",
        "--out-dir",
        path(&out_dir),
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let git_out = temp_dir("cli-git-export-no-user-out");
    let out = export(&[
        "export",
        "--format",
        "git",
        "--repo-dir",
        path(&repo.join("missing")),
        "--project",
        "ovzdusie",
        "--out-dir",
        path(&git_out),
    ]);
    assert!(!out.status.success(), "{out:?}");
}
