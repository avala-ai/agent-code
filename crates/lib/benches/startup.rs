use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

use agent_code_lib::config::Config;

fn bench_config_load(c: &mut Criterion) {
    let mut group = c.benchmark_group("startup");

    group.bench_function("config_load", |b| {
        b.iter(|| {
            black_box(Config::load().unwrap());
        });
    });

    group.bench_function("config_default", |b| {
        b.iter(|| {
            black_box(Config::default());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_config_load);
criterion_main!(benches);
