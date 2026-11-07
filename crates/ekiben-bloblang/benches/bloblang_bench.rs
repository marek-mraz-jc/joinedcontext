use criterion::{criterion_group, criterion_main, Criterion};
use ekiben_bloblang::Environment;
use std::hint::black_box;

fn bench_bloblang(c: &mut Criterion) {
    let env = Environment::new();
    let mapping_src = r#"
map clean {
  root = this.trim().lowercase()
}
root.id = "urn:ngsi-ld:PointOfInterest:org:space:" + this.id.string()
root.name = this.name.apply("clean")
root.tags = this.tags.filter(t -> t.length() > 2).map_each(t -> t.uppercase()).sort()
root.score = (this.a * 2 + this.b) / 3
"#;

    c.bench_function("bloblang_parse", |b| {
        b.iter(|| {
            let _ = env.compile_mapping(black_box(mapping_src)).unwrap();
        })
    });

    let exec = env.compile_mapping(mapping_src).unwrap();
    let payload = br#"{"id":12345,"name":"  Prague Castle  ","tags":["cz","unesco","landmark","prague"],"a":10,"b":25}"#;

    c.bench_function("bloblang_exec", |b| {
        b.iter(|| {
            let _ = exec.execute_bytes(black_box(payload)).unwrap();
        })
    });
}

criterion_group!(benches, bench_bloblang);
criterion_main!(benches);
