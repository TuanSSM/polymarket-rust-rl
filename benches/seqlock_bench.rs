use criterion::{black_box, criterion_group, criterion_main, Criterion};
use polymarket_rust_rl::seqlock::SeqLock;
use polymarket_rust_rl::types::Parameters;

fn bench_seqlock_read(c: &mut Criterion) {
    let lock = SeqLock::new(Parameters::default());
    c.bench_function("seqlock_read", |b| b.iter(|| black_box(lock.read())));
}

fn bench_seqlock_write(c: &mut Criterion) {
    let lock = SeqLock::new(Parameters::default());
    let params = Parameters {
        max_position: 200.0,
        risk_limit: 0.1,
        spread_threshold: 0.003,
        momentum_window: 30,
    };
    c.bench_function("seqlock_write", |b| {
        b.iter(|| lock.write(black_box(params)))
    });
}

fn bench_seqlock_read_with_seq(c: &mut Criterion) {
    let lock = SeqLock::new(Parameters::default());
    c.bench_function("seqlock_read_with_seq", |b| {
        b.iter(|| black_box(lock.read_with_seq()))
    });
}

criterion_group!(
    benches,
    bench_seqlock_read,
    bench_seqlock_write,
    bench_seqlock_read_with_seq
);
criterion_main!(benches);
