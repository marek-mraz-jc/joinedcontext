//! A batch holds no copy it does not need (jc-ekiben-batch-mem).
//!
//! One bbsk run of 14,961 municipality polygons held ~1 GiB in jc's one process: every processor
//! serialised and cached each structured part before it ran, `content()` on a structured-only
//! part leaked its JSON, split cached the bytes it only measured, and a mapping held its whole
//! input batch beside its whole output. This binary counts every byte the process allocates, so
//! it holds one test: its peak is process-wide.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ekiben_bloblang::Environment;
use ekiben_core::{
    execute_all, ComponentConfig, OrgId, Part, PartsView, ProcContext, Registry, ResourceManager,
    StreamMetrics,
};
use serde_json::json;
use tokio::sync::watch;

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

fn now() -> usize {
    NOW.load(Ordering::Relaxed)
}

fn reset_peak() -> usize {
    let now = now();
    PEAK.store(now, Ordering::Relaxed);
    now
}

fn make_ctx() -> (Arc<Registry>, ProcContext) {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("org"));
    let (_tx, rx) = watch::channel(false);
    let ctx = ProcContext {
        stream_id: "test-stream".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("test-stream", "org"),
        label: "root".to_string(),
        component_type: "root".to_string(),
        path: "pipeline.processors".to_string(),
        file_root: None,
        shutdown_rx: rx,
    };
    (reg, ctx)
}

fn procs(
    conf: serde_json::Value,
    reg: &Registry,
    ctx: &ProcContext,
) -> Vec<Arc<dyn ekiben_core::Processor>> {
    let conf = ComponentConfig::from_json_value(&conf, "noop", None).unwrap();
    reg.build_processor_list(&[conf], ctx).unwrap()
}

/// A part of `n` numbers, held only as a structured value (a mapping's result).
fn structured(n: usize) -> Part {
    Part::from_json(json!({ "v": (0..n).collect::<Vec<_>>() }))
}

#[test]
fn a_batch_holds_no_copy_it_does_not_need() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (reg, ctx) = make_ctx();

        // A parsed part keeps its bytes: content() returns them unchanged, spacing and `1.0`
        // included, and the part still holds its parsed value.
        let text = r#"{ "a" : 1.0, "b": [1,2] }"#;
        let mut parsed = Part::from_str(text);
        parsed.as_structured().unwrap();
        let mapping = procs(json!({ "mapping": "root = content()" }), &reg, &ctx);
        let out = execute_all(&ctx, &mapping, vec![vec![parsed.clone()]])
            .await
            .unwrap();
        assert_eq!(&out[0][0].as_bytes_ref()[..], text.as_bytes());
        assert!(parsed.holds_structured());
        assert_eq!(&parsed.as_bytes_ref()[..], text.as_bytes());

        // content() on a structured-only part allocates nothing that outlives the call.
        let exec = Environment::new()
            .compile_mapping("root = content()")
            .unwrap();
        let batch = vec![structured(1_000)];
        let view = PartsView(&batch);
        let mut meta = BTreeMap::new();
        drop(exec.map_part(0, &view, &mut meta, None).unwrap());
        let before = now();
        for _ in 0..200 {
            drop(exec.map_part(0, &view, &mut meta, None).unwrap());
        }
        let lasting = now().saturating_sub(before);
        assert!(lasting < 1024, "200 content() calls left {lasting} bytes");
        drop(batch);

        // A mapping over N parts frees each input once its output exists: the peak stays near
        // the input batch, never input plus output (each output is as large as its input and
        // shares nothing with it).
        let mapping = procs(
            json!({ "mapping": "root.w = this.v.map_each(x -> x + 1)" }),
            &reg,
            &ctx,
        );
        let base = now();
        let batch: Vec<Part> = (0..64).map(|_| structured(2_000)).collect();
        let input = now() - base;
        reset_peak();
        let out = execute_all(&ctx, &mapping, vec![batch]).await.unwrap();
        let peak = PEAK.load(Ordering::Relaxed) - base;
        assert_eq!(out[0].len(), 64);
        assert!(
            peak < input * 3 / 2,
            "mapping peaked at {peak} bytes over an input of {input}"
        );
        drop(out);

        // split measures byte_size without caching any part's JSON, and cuts where it did.
        let sizes = [300usize, 500, 200, 700, 100, 400, 900, 50];
        let batch: Vec<Part> = sizes
            .iter()
            .map(|&n| Part::from_json(json!({ "s": "x".repeat(n * 40) })))
            .collect();
        let lens: Vec<usize> = batch.iter().map(|p| p.as_bytes_ref().len()).collect();
        let json_total: usize = lens.iter().sum();
        let limit = 40_000;
        let mut expected = vec![vec![]];
        let mut run = 0;
        for (i, &len) in lens.iter().enumerate() {
            if run + len > limit && !expected.last().unwrap().is_empty() {
                expected.push(vec![]);
                run = 0;
            }
            expected.last_mut().unwrap().push(i);
            run += len;
        }
        let split = procs(
            json!({ "split": { "size": 0, "byte_size": limit } }),
            &reg,
            &ctx,
        );
        let before = now();
        let out = execute_all(&ctx, &split, vec![batch]).await.unwrap();
        let grown = now().saturating_sub(before);
        let got: Vec<usize> = out.iter().map(Vec::len).collect();
        assert_eq!(got, expected.iter().map(Vec::len).collect::<Vec<_>>());
        assert!(
            grown < json_total / 4,
            "split kept {grown} bytes beside {json_total} bytes of JSON"
        );
    });
}
