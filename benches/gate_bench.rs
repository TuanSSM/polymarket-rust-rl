use criterion::{black_box, criterion_group, criterion_main, Criterion};
use polymarket_rust_rl::gate;

fn bench_position_limit_gate(c: &mut Criterion) {
    c.bench_function("position_limit_gate", |b| {
        b.iter(|| gate::position_limit_gate(black_box(50.0), black_box(100.0)))
    });
}

fn bench_evaluate_gates(c: &mut Criterion) {
    c.bench_function("evaluate_gates", |b| {
        b.iter(|| {
            gate::evaluate_gates(
                black_box(50.0),
                black_box(100.0),
                black_box(0.05),
                black_box(0.01),
                black_box(0.001),
                black_box(0.05),
            )
        })
    });
}

fn bench_full_gate_eval(c: &mut Criterion) {
    c.bench_function("full_gate_eval", |b| {
        b.iter(|| {
            gate::full_gate_eval(
                black_box(50.0),
                black_box(100.0),
                black_box(0.05),
                black_box(0.01),
                black_box(0.001),
                black_box(0.05),
                black_box(1.0),
                black_box(1.0),
            )
        })
    });
}

criterion_group!(
    benches,
    bench_position_limit_gate,
    bench_evaluate_gates,
    bench_full_gate_eval
);
criterion_main!(benches);
