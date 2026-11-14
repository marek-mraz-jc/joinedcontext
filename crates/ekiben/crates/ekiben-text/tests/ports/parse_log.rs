// Rust twins of Bento 1.22.0's processor_parse_log_test.go: same case names, same inputs.

use crate::common::{build_yaml, bytes, run};
use chrono::Datelike;

// processor_parse_log_test.go

#[tokio::test]
async fn test_parse_log_cases() {
    let year = chrono::Utc::now().year();
    let cases = [
        (
            "valid syslog_rfc5424 input, valid json output",
            "syslog_rfc5424",
            r#"<42>4 2049-10-11T22:14:15.003Z toaster.smarthome myapp - 2 [home01 device_id="43"] failed to make a toast."#,
            r#"{"appname":"myapp","facility":5,"hostname":"toaster.smarthome","message":"failed to make a toast.","msgid":"2","priority":42,"severity":2,"structureddata":{"home01":{"device_id":"43"}},"timestamp":"2049-10-11T22:14:15.003Z","version":4}"#.to_string(),
        ),
        (
            "invalid syslog_rfc5424 input, invalid json output",
            "syslog_rfc5424",
            "not a syslog at all.",
            "not a syslog at all.".to_string(),
        ),
        (
            "valid syslog_rfc3164 input, valid json output",
            "syslog_rfc3164",
            "<28>Dec  2 16:49:23 host app[23410]: Test",
            format!(r#"{{"appname":"app","facility":3,"hostname":"host","message":"Test","priority":28,"procid":"23410","severity":4,"timestamp":"{year}-12-02T16:49:23Z"}}"#),
        ),
    ];
    for (name, format, input, output) in cases {
        let out = run(
            &format!("parse_log:\n  format: {format}\n  best_effort: true\n"),
            &[input],
        )
        .await;
        assert_eq!(out.len(), 1, "{name}");
        assert_eq!(out[0].len(), 1, "{name}");
        assert_eq!(bytes(&out[0][0]), output, "{name}");
    }
}

#[tokio::test]
async fn test_parse_log_rfc5424() {
    let cases = [
        (
            "valid syslog_rfc5424 1",
            r#"<42>4 2049-10-11T22:14:15.003Z toaster.smarthome myapp - 2 [home01 device_id="43"] failed to make a toast."#,
            r#"{"appname":"myapp","facility":5,"hostname":"toaster.smarthome","message":"failed to make a toast.","msgid":"2","priority":42,"severity":2,"structureddata":{"home01":{"device_id":"43"}},"timestamp":"2049-10-11T22:14:15.003Z","version":4}"#,
        ),
        (
            "valid syslog_rfc5424 2",
            r#"<23>4 2032-10-11T22:14:15.003Z foo.bar baz - 10 [home02 device_id="44"] test log."#,
            r#"{"appname":"baz","facility":2,"hostname":"foo.bar","message":"test log.","msgid":"10","priority":23,"severity":7,"structureddata":{"home02":{"device_id":"44"}},"timestamp":"2032-10-11T22:14:15.003Z","version":4}"#,
        ),
    ];
    for (name, input, output) in cases {
        let mut out = run(
            "parse_log:\n  format: syslog_rfc5424\n  best_effort: true\n",
            &[input],
        )
        .await;
        assert_eq!(out.len(), 1, "{name}");
        let p = &mut out[0][0];
        assert_eq!(bytes(p), output, "{name}");
        // Bento's check: every structureddata element is an object.
        let doc = p.as_structured().expect("structured");
        let sd = doc.to_json_value()["structureddata"].clone();
        assert!(
            sd.as_object()
                .is_some_and(|o| o.values().all(|v| v.is_object())),
            "{name}"
        );
    }
}

#[tokio::test]
async fn parse_log_errors_and_config_refusals() {
    let out = run(
        "parse_log:\n  format: syslog_rfc5424\n",
        &["<12>1 2003-02-30T05:14:15Z h a p m -", "<12>", ""],
    )
    .await;
    let errs: Vec<String> = out[0]
        .iter()
        .map(|p| p.error_get().map(|e| e.to_string()).unwrap_or_default())
        .collect();
    assert_eq!(
        errs,
        [
            "parsing time \"2003-02-30T05:14:15Z\": day out of range [col 26]",
            "expecting a version value in the range 1-999 [col 4]",
            "expecting a priority value within angle brackets [col 0]",
        ]
    );
    for (yaml, want) in [
        ("parse_log:\n  format: nope\n", "format not recognised: nope"),
        (
            "parse_log:\n  format: syslog_rfc3164\n  default_year: twenty\n",
            "failed to convert year twenty into integer:  strconv.Atoi: parsing \"twenty\": invalid syntax",
        ),
        (
            "parse_log:\n  format: syslog_rfc3164\n  default_timezone: Mars/Base\n",
            "failed to lookup timezone UTC - unknown time zone Mars/Base",
        ),
        ("parse_log:\n  format: syslog_rfc3164\n  nope: 1\n", "field nope not recognised"),
    ] {
        let err = build_yaml(yaml).err().expect(yaml).to_string();
        assert!(err.contains(want), "{yaml}: {err}");
    }
}
