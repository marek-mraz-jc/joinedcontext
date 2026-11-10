//! A run of any size holds one page of its source at a time (jc-ekiben-paging).
//!
//! A pipeline used to run every processor over its whole source as one batch: a 14,961-row
//! ArcGIS layer kept ~850 MiB of mapped rows live in jc's one process, and a source past the
//! member bound failed. This binary counts every byte the process allocates, so it holds one
//! test: its peak is process-wide.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use ekiben_core::{OrgId, OrgQuotas, ResourceManager, StreamConfig};
use ekiben_server::run_stream_once;
use ekiben_testkit::full_registry;

/// Counts the bytes allocated now and the most at any moment since [`reset_peak`].
struct Counting;

static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes to `System` unchanged; the counters only observe the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let at = unsafe { System.alloc(layout) };
        if !at.is_null() {
            grew(layout.size());
        }
        at
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let at = unsafe { System.alloc_zeroed(layout) };
        if !at.is_null() {
            grew(layout.size());
        }
        at
    }

    unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
        unsafe { System.dealloc(at, layout) };
        NOW.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, at: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(at, layout, size) };
        if !moved.is_null() {
            NOW.fetch_sub(layout.size(), Ordering::Relaxed);
            grew(size);
        }
        moved
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn grew(size: usize) {
    let now = NOW.fetch_add(size, Ordering::Relaxed).saturating_add(size);
    PEAK.fetch_max(now, Ordering::Relaxed);
}

fn reset_peak() -> usize {
    let now = NOW.load(Ordering::Relaxed);
    PEAK.store(now, Ordering::Relaxed);
    now
}

const ROWS: usize = 200_000;

/// Live bytes the run may add at its peak: the source text (~4 MiB), a few pages of mapped rows
/// (~4 MiB each) and the runtime. The rows mapped at once, as one batch, take ~600 MiB.
const BOUND: usize = 64 << 20;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_200000_row_source_runs_in_pages_within_a_fixed_memory_bound() {
    // One csv document of 200,000 rows, as an http source answers it; each row becomes an
    // NGSI-LD entity of several attributes, written 1,000 to a line.
    let yaml = format!(
        r#"
run: once
org: big
input:
  generate:
    count: 1
    interval: ""
    mapping: 'root = "id,name,group\n" + range(0, {ROWS}).map_each(i -> "%d,row %d,%d".format(i, i, i % 97)).join("\n")'
pipeline:
  processors:
    - unarchive:
        format: csv
    - mapping: |
        root = {{
          "id": "urn:ngsi-ld:Thing:" + this.id,
          "type": "Thing",
          "name": {{ "type": "Property", "value": this.name }},
          "group": {{ "type": "Property", "value": this.group.number() }},
          "description": {{ "type": "Property", "value": "Row %s of group %s, a text long enough to weigh like a real attribute".format(this.id, this.group) }},
          "location": {{ "type": "GeoProperty", "value": {{ "type": "Point", "coordinates": [17.1, 48.1] }} }},
          "dataProvider": {{ "type": "Property", "value": "paging test" }}
        }}
    - mapping: 'root = this.id'
    - split:
        size: 1000
    - archive:
        format: lines
output:
  file:
    path: out.txt
    codec: lines
"#
    );
    let resources = ResourceManager::new();
    // Quotas out of the way: what this measures is the memory, not a refusal.
    resources.set_org_quotas(
        &OrgId::new("big"),
        OrgQuotas {
            max_in_flight_messages: 10 * ROWS,
            max_memory_bytes_per_run: usize::MAX,
            max_cpu_time_ms_per_run: 3_600_000,
            ..Default::default()
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let conf = StreamConfig::from_yaml_or_json(&yaml).unwrap();

    let before = reset_peak();
    let summary = run_stream_once(
        "big",
        conf,
        full_registry(),
        resources,
        Some(dir.path().to_path_buf()),
        false,
    )
    .await
    .unwrap();
    let peak = PEAK.load(Ordering::Relaxed) - before;
    eprintln!("peak above start: {peak} bytes ({} MiB)", peak >> 20);

    assert!(summary.success, "{:?}", summary.error);
    let text = std::fs::read_to_string(dir.path().join("out.txt")).unwrap();
    let ids: Vec<&str> = text.lines().collect();
    assert_eq!(ids.len(), ROWS);
    assert_eq!(ids[0], "urn:ngsi-ld:Thing:0");
    assert_eq!(ids[ROWS - 1], format!("urn:ngsi-ld:Thing:{}", ROWS - 1));
    assert_eq!(summary.messages_out, (ROWS / 1000) as u64);
    assert!(
        peak < BOUND,
        "the run peaked {peak} bytes above its start, over the {BOUND} bound"
    );
}
