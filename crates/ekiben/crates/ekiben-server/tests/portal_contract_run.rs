//! Contract with the Portal, end to end: the streams the Portal renders for the seed pipelines
//! of `tests/e2e-parity` (whose sources have recorded answers) run once in ekiben's runner,
//! posted through the Portal's calls, and write NGSI-LD to a stub gateway with the token the
//! runner's sidecar issues to that one pipeline (client credentials, client id
//! `{project}/{pipeline}`, no secret: PL-19). Each entity Bento 1.22 wrote for the pipeline is
//! either written by ekiben or refused by the Portal's validation stage (and then posted to the
//! Portal's rejected route), and the run's outcome reaches the Portal's internal listener.

mod portal_contract_common;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use portal_contract_common::{cases, deploy, rewrite, runner, stub, ADMIN_TOKEN};
use serde_json::Value;

/// The entity ids Bento wrote for a parity case (`tests/e2e-parity/<project>__<name>/case.json`).
fn bento_ids(project: &str, name: &str) -> Option<BTreeSet<String>> {
    let path = portal_contract_common::root()
        .join("tests/e2e-parity")
        .join(format!("{project}__{name}/case.json"));
    let case: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let mut ids = BTreeSet::new();
    for r in case["bento"]["requests"].as_array()? {
        let r: Value = serde_json::from_str(r.as_str()?).ok()?;
        if r["route"]
            .as_str()
            .is_some_and(|x| x.ends_with("entityOperations/upsert"))
        {
            for e in r["body"].as_array().into_iter().flatten() {
                ids.insert(e["id"].as_str()?.to_string());
            }
        }
    }
    Some(ids)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn portal_streams_run_once_and_write_ngsild_with_the_pipeline_token() {
    let (stub_url, seen) = stub().await;
    for (k, v) in [
        ("JC_GATEWAY_URL", format!("{stub_url}/gw")),
        ("JC_PIPELINE_TOKEN_URL", format!("{stub_url}/token")),
        ("JC_TOKEN_URL", format!("{stub_url}/token")),
        (
            "JC_GATEWAY_HOST",
            stub_url.trim_start_matches("http://").to_string(),
        ),
        ("JC_ORG_DOMAIN", "example.org".to_string()),
    ] {
        std::env::set_var(format!("EKIBEN_VAR_{k}"), v);
    }
    // The stub is on loopback: this organization may reach private addresses.
    let (runner, _root) = runner(true).await;
    let client = reqwest::Client::new();
    let mut checked = 0;
    let mut problems = Vec::new();
    for case in cases() {
        let Some(expected) = bento_ids(&case.project, &case.name) else {
            continue;
        };
        checked += 1;
        let mut stream = case.stream.clone();
        rewrite(&mut stream, &stub_url, "tcp://127.0.0.1:9");
        let before = {
            let s = seen.lock().unwrap();
            (s.upserts.len(), s.internal.len(), s.tokens.len())
        };
        if let Err(why) = deploy(&client, &runner, &case, &stream).await {
            problems.push(format!("{}: {why}", case.id));
            continue;
        }
        // The run queue's poll (`Runner::state`): Finished when the runner says not active.
        let url = format!("{runner}/streams/{}", case.id);
        let started = Instant::now();
        loop {
            let one: Value = client
                .get(&url)
                .bearer_auth(ADMIN_TOKEN)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if one["active"] == Value::Bool(false) {
                break;
            }
            if started.elapsed() > Duration::from_secs(120) {
                problems.push(format!("{}: still running after 120 s", case.id));
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = client.delete(&url).bearer_auth(ADMIN_TOKEN).send().await;

        let s = seen.lock().unwrap();
        let pipeline = format!("{}/{}", case.project, case.name);
        let token = format!("tok:{pipeline}");
        let upserts = &s.upserts[before.0..];
        let internal = &s.internal[before.1..];
        if !s.missing.is_empty() {
            problems.push(format!(
                "{}: sources without a recorded answer: {:?}",
                case.id, s.missing
            ));
        }
        if !s.tokens[before.2..]
            .iter()
            .all(|(id, grant)| *id == pipeline && grant == "client_credentials")
        {
            problems.push(format!(
                "{}: token requests {:?}",
                case.id,
                &s.tokens[before.2..]
            ));
        }
        let mut written = BTreeSet::new();
        for (bearer, _slug, _query, body) in upserts {
            if *bearer != token {
                problems.push(format!(
                    "{}: an upsert presented {bearer}, not the pipeline's token",
                    case.id
                ));
            }
            for e in body.as_array().into_iter().flatten() {
                let id = e["id"].as_str().unwrap_or_default();
                let ty = e["type"].as_str().unwrap_or_default();
                if !id.starts_with(&format!("urn:ngsi-ld:{ty}:")) {
                    problems.push(format!(
                        "{}: an upserted entity is not NGSI-LD: {id} {ty}",
                        case.id
                    ));
                }
                written.insert(id.to_string());
            }
        }
        let mut rejected = BTreeSet::new();
        let mut outcome_sent = BTreeSet::new();
        for (bearer, path, body) in internal {
            if *bearer != token {
                problems.push(format!("{}: {path} presented {bearer}", case.id));
            }
            if path.ends_with("/rejected") {
                if rejected.is_empty() {
                    eprintln!("{}: first refusal: {}", case.id, body["error"]);
                }
                rejected.insert(
                    body["record"]["id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            if path.ends_with("/outcomes") {
                for item in body
                    .as_array()
                    .map(|a| a.to_vec())
                    .unwrap_or_else(|| vec![body.clone()])
                {
                    for id in item["sent"].as_array().into_iter().flatten() {
                        outcome_sent.insert(id.as_str().unwrap_or_default().to_string());
                    }
                }
            }
        }
        let accounted: BTreeSet<String> = written.union(&rejected).cloned().collect();
        if accounted != expected {
            let lost: Vec<_> = expected.difference(&accounted).take(3).collect();
            let extra: Vec<_> = accounted.difference(&expected).take(3).collect();
            problems.push(format!(
                "{}: Bento wrote {} entities, ekiben wrote {} and the stage refused {}; missing {lost:?}, extra {extra:?}",
                case.id,
                expected.len(),
                written.len(),
                rejected.len()
            ));
        }
        if outcome_sent != written {
            problems.push(format!(
                "{}: the outcome reported {} sent, the gateway got {}",
                case.id,
                outcome_sent.len(),
                written.len()
            ));
        }
        eprintln!(
            "{}: {} written, {} refused by the stage, {} upserts",
            case.id,
            written.len(),
            rejected.len(),
            upserts.len()
        );
    }
    assert!(
        checked >= 20,
        "the parity cases are in the fixture ({checked})"
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
