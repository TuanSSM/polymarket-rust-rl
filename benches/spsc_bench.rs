use criterion::{black_box, criterion_group, criterion_main, Criterion};
use polymarket_rust_rl::spsc::SpscRingBuffer;
use polymarket_rust_rl::types::{Side, TradeEvent};

fn make_event(i: u64) -> TradeEvent {
    TradeEvent {
        timestamp_ns: i,
        price: 0.55,
        quantity: 10.0,
        side: Side::Buy,
        market_id: 0,
    }
}

fn bench_spsc_push_pop(c: &mut Criterion) {
    let rb = SpscRingBuffer::<TradeEvent>::new(1024);
    let (prod, cons) = rb.split();
    c.bench_function("spsc_push_pop", |b| {
        b.iter(|| {
            prod.try_push(black_box(make_event(1))).unwrap();
            black_box(cons.try_pop().unwrap());
        })
    });
}

fn bench_spsc_burst(c: &mut Criterion) {
    let rb = SpscRingBuffer::<TradeEvent>::new(1024);
    let (prod, cons) = rb.split();
    c.bench_function("spsc_burst_64", |b| {
        b.iter(|| {
            for i in 0..64 {
                prod.try_push(black_box(make_event(i))).unwrap();
            }
            for _ in 0..64 {
                black_box(cons.try_pop().unwrap());
            }
        })
    });
}

fn bench_spsc_drain(c: &mut Criterion) {
    let rb = SpscRingBuffer::<TradeEvent>::new(1024);
    let (prod, cons) = rb.split();
    let mut buf = Vec::with_capacity(64);
    c.bench_function("spsc_drain_64", |b| {
        b.iter(|| {
            for i in 0..64 {
                prod.try_push(black_box(make_event(i))).unwrap();
            }
            buf.clear();
            cons.drain_into(&mut buf, 64);
            black_box(&buf);
        })
    });
}

criterion_group!(benches, bench_spsc_push_pop, bench_spsc_burst, bench_spsc_drain);
criterion_main!(benches);
