//! The parity corpus against the generated machines' state charts (T-7677). The machines are
//! go-syslog's Ragel goto code translated line by line, so a test per line is no proof; instead the
//! corpus in tests/parity/cases.json, whose outputs Bento recorded, must take every transition of
//! each chart: from every state on every byte class (by the state it leads to and the error it
//! sets) and the end of input in every state (its EOF actions).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{json, Value};

/// A machine run over `input` with its EOF actions off: the state it ends in and the error it set.
pub(super) type Step<'a> = &'a dyn Fn(&[u8]) -> (i32, Option<String>);

/// One transition: the state it leaves, the state it enters (None: the input ended there) and the
/// error it sets without its column.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Edge {
    from: i32,
    to: Option<i32>,
    error: String,
}

/// The error a step set, without its column; empty when the step left the error as it was.
fn kind(before: &Option<String>, after: Option<String>) -> String {
    match after {
        Some(err) if Some(&err) != before.as_ref() => {
            err.split(" [col ").next().unwrap_or_default().to_string()
        }
        _ => String::new(),
    }
}

/// Every transition the chart has from START, each with the shortest input that takes it.
pub(super) fn explore(step: Step) -> BTreeMap<Edge, Vec<u8>> {
    let mut edges = BTreeMap::new();
    let (start, none) = step(b"");
    let mut seen = BTreeSet::from([start]);
    let mut queue = VecDeque::from([(start, Vec::new(), none)]);
    while let Some((from, prefix, before)) = queue.pop_front() {
        edges
            .entry(Edge {
                from,
                to: None,
                error: String::new(),
            })
            .or_insert_with(|| prefix.clone());
        for byte in 0..=u8::MAX {
            let mut input = prefix.clone();
            input.push(byte);
            let (to, err) = step(&input);
            let edge = Edge {
                from,
                to: Some(to),
                error: kind(&before, err.clone()),
            };
            edges.entry(edge).or_insert_with(|| input.clone());
            if seen.insert(to) {
                queue.push_back((to, input, err));
            }
        }
    }
    edges
}

/// The transitions `input` takes, the end of input included.
pub(super) fn walk(step: Step, input: &[u8]) -> Vec<Edge> {
    let mut edges = Vec::with_capacity(input.len() + 1);
    let (mut from, mut before) = step(b"");
    for end in 1..=input.len() {
        let (to, err) = step(&input[..end]);
        edges.push(Edge {
            from,
            to: Some(to),
            error: kind(&before, err.clone()),
        });
        (from, before) = (to, err);
    }
    edges.push(Edge {
        from,
        to: None,
        error: String::new(),
    });
    edges
}

/// The inputs of the parity cases whose processor is exactly `processor`.
pub(super) fn corpus(processor: &Value) -> Vec<Vec<u8>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/parity/cases.json");
    let text = std::fs::read_to_string(path).expect("tests/parity/cases.json");
    let cases: Vec<Value> = serde_json::from_str(&text).expect("cases.json is JSON");
    cases
        .iter()
        .filter(|c| &c["processor"] == processor)
        .map(|c| match c["input_hex"].as_str() {
            Some(h) => hex::decode(h).expect("input_hex"),
            None => c["input"].as_str().unwrap_or_default().as_bytes().to_vec(),
        })
        .collect()
}

/// Cases for the chart's transitions no corpus input of `processor` takes, to record with
/// scripts/record-text-parity.py (one JSON object per line, named `<label> chart <first edge>`):
/// the longest inputs first, each kept only when it takes a transition no kept one takes.
pub(super) fn missing(
    step: Step,
    corpus: &[Vec<u8>],
    processor: &Value,
    label: &str,
) -> Vec<String> {
    let taken: BTreeSet<Edge> = corpus.iter().flat_map(|i| walk(step, i)).collect();
    let mut needed: BTreeMap<Edge, Vec<u8>> = explore(step)
        .into_iter()
        .filter(|(edge, _)| !taken.contains(edge))
        .collect();
    let mut inputs: Vec<Vec<u8>> = needed.values().cloned().collect();
    inputs.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    let mut cases = Vec::new();
    for input in inputs {
        let new: Vec<Edge> = walk(step, &input)
            .into_iter()
            .filter(|e| needed.remove(e).is_some())
            .collect();
        let Some(edge) = new.first() else { continue };
        let name = format!(
            "{label} chart {} {} {}",
            edge.from,
            edge.to.map_or("end".to_string(), |t| t.to_string()),
            if edge.error.is_empty() {
                "ok"
            } else {
                &edge.error
            }
        );
        let case = match String::from_utf8(input.clone()) {
            Ok(text) => json!({"name": name, "processor": processor, "input": text}),
            Err(_) => {
                json!({"name": name, "processor": processor, "input_hex": hex::encode(&input)})
            }
        };
        cases.push(case.to_string());
    }
    cases
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-state chart: 'a' moves 1 to 2, anything else fails to 0 with an error.
    fn toy(input: &[u8]) -> (i32, Option<String>) {
        let mut cs = 1;
        let mut err = None;
        for (at, b) in input.iter().enumerate() {
            match (cs, b) {
                (1, b'a') => cs = 2,
                (0, _) => {}
                _ => {
                    cs = 0;
                    err = Some(format!("bad [col {at}]"));
                }
            }
        }
        (cs, err)
    }

    #[test]
    fn explore_finds_every_class_with_its_shortest_input() {
        let edges = explore(&toy);
        let got: Vec<(i32, Option<i32>, &str, &[u8])> = edges
            .iter()
            .map(|(e, i)| (e.from, e.to, e.error.as_str(), i.as_slice()))
            .collect();
        assert_eq!(
            got,
            vec![
                (0, None, "", &b"\x00"[..]),
                (0, Some(0), "", &b"\x00\x00"[..]),
                (1, None, "", &b""[..]),
                (1, Some(0), "bad", &b"\x00"[..]),
                (1, Some(2), "", &b"a"[..]),
                (2, None, "", &b"a"[..]),
                (2, Some(0), "bad", &b"a\x00"[..]),
            ]
        );
    }

    #[test]
    fn missing_keeps_the_longest_inputs_that_take_something_new() {
        let processor = json!({"toy": {}});
        // "a" alone takes 1->2 and the end in 2. "\0\0" then takes 1->0, 0->0 and the end in 0,
        // "a\0" takes 2->0, "\0" nothing new, and "" the end in 1.
        let cases = missing(&toy, &[b"a".to_vec()], &processor, "toy");
        assert_eq!(
            cases,
            vec![
                r#"{"name":"toy chart 1 0 bad","processor":{"toy":{}},"input":"\u0000\u0000"}"#,
                r#"{"name":"toy chart 2 0 bad","processor":{"toy":{}},"input":"a\u0000"}"#,
                r#"{"name":"toy chart 1 end ok","processor":{"toy":{}},"input":""}"#,
            ]
        );
        assert!(missing(
            &toy,
            &[b"a".to_vec(), b"a\0".to_vec(), b"\0\0".to_vec(), Vec::new()],
            &processor,
            "toy"
        )
        .is_empty());
        let bytes = missing(
            &|i: &[u8]| (i.first().map_or(1, |b| 2 + i32::from(*b >= 0x80)), None),
            &[],
            &processor,
            "hex",
        );
        assert!(
            bytes.iter().any(|c| c.contains(r#""input_hex":"8000""#)),
            "{bytes:?}"
        );
    }

    #[test]
    fn walk_names_each_step_and_the_end() {
        let edges = walk(&toy, b"ab");
        let got: Vec<(i32, Option<i32>, &str)> = edges
            .iter()
            .map(|e| (e.from, e.to, e.error.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![(1, Some(2), ""), (2, Some(0), "bad"), (0, None, "")]
        );
    }
}
