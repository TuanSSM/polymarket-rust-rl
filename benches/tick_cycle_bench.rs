use criterion::{black_box, criterion_group, criterion_main, Criterion};
use polymarket_rust_rl::controller::create_infrastructure;
use polymarket_rust_rl::core_engine::CoreEngine;
use polymarket_rust_rl::types::{Side, TradeEvent};

fn bench_full_tick_cycle(c: &mut Criterion) {
    let (buffers, params) = create_infrastructure(1, 1024);
    let (prod, cons) = buffers[0].split();
    let mut engine = CoreEngine::new(0, 0, cons, params);
    let mut signals = Vec::new();

    c.bench_function("full_tick_cycle_1core", |b| {
        b.iter(|| {
            let event = TradeEvent {
                timestamp_ns: 1,
                price: black_box(0.55),
                quantity: black_box(10.0),
                side: Side::Buy,
                market_id: 0,
            };
            prod.try_push(event).unwrap();
            engine.process_tick();
            engine.drain_signals(&mut signals);
            signals.clear();
        })
    });
}

fn bench_signal_generation(c: &mut Criterion) {
    let (buffers, params) = create_infrastructure(1, 1024);
    let (prod, cons) = buffers[0].split();
    let mut engine = CoreEngine::new(0, 0, cons, params);
    let mut signals = Vec::new();
    let mut price = 0.50;

    c.bench_function("signal_generation_varying_price", |b| {
        b.iter(|| {
            price += 0.001;
            if price > 0.60 {
                price = 0.50;
            }
            let event = TradeEvent {
                timestamp_ns: 1,
                price: black_box(price),
                quantity: black_box(10.0),
                side: Side::Buy,
                market_id: 0,
            };
            prod.try_push(event).unwrap();
            engine.process_tick();
            engine.drain_signals(&mut signals);
            signals.clear();
        })
    });
}

criterion_group!(benches, bench_full_tick_cycle, bench_signal_generation);
criterion_main!(benches);
