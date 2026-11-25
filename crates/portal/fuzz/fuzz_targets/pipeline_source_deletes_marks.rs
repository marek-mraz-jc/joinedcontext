//! What a source-delete stream or an events stream posts back (PL-97, PL-98): every id that may
//! be put in a delete is one plain URI, and the stream a list of such ids renders into is a JSON
//! object whatever the ids were.
#![no_main]

use joinedcontext_portal::pipeline_source_deletes::{
    deletable, render, Cause, Marks, Report, MAX_PER_STREAM,
};
use libfuzzer_sys::fuzz_target;

fn assert_plain(id: &str) {
    assert!(id.contains(':') && id.len() <= 2048, "{id:?}");
    assert!(
        !id.chars()
            .any(|c| !c.is_ascii_graphic() || matches!(c, '"' | '\\' | '$' | '{' | '}')),
        "{id:?}"
    );
}

fuzz_target!(|data: &[u8]| {
    if let Ok(marks) = serde_json::from_slice::<Marks>(data) {
        for id in marks.ids.iter().filter(|id| deletable(id)) {
            assert_plain(id);
        }
        let _ = (marks.run, marks.failed.len());
    }
    if let Ok(report) = serde_json::from_slice::<Report>(data) {
        let _ = (
            report.run,
            report.deleted.len() + report.gone.len() + report.edited.len(),
            report.failed.len(),
        );
    }
    if let Ok(ids) = serde_json::from_slice::<Vec<String>>(data) {
        let ids: Vec<String> = ids
            .into_iter()
            .filter(|id| deletable(id))
            .take(MAX_PER_STREAM)
            .collect();
        for id in &ids {
            assert_plain(id);
        }
        let wrote = vec![0i64; ids.len()];
        let stream = render(
            "p",
            "pl",
            "slug",
            ("run", Cause::Snapshot),
            (&ids, &wrote),
            "http://report",
        );
        assert!(stream.is_object());
    }
});
