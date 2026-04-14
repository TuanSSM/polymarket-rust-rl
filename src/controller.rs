use crate::seqlock::SeqLock;
use crate::spsc::{Producer, SpscRingBuffer};
use crate::types::{Parameters, Signal, TradeEvent};
use std::sync::Arc;

/// Multi-core orchestrator. Publishes parameters via SeqLock,
/// distributes trade events to per-core SPSC channels, and
/// drains signals from core engines.
pub struct Controller<'a> {
    producers: Vec<Producer<'a, TradeEvent>>,
    params: Arc<SeqLock<Parameters>>,
    collected_signals: Vec<Signal>,
    events_published: u64,
}

impl<'a> Controller<'a> {
    pub fn new(
        producers: Vec<Producer<'a, TradeEvent>>,
        params: Arc<SeqLock<Parameters>>,
    ) -> Self {
        Self {
            producers,
            params,
            collected_signals: Vec::with_capacity(256),
            events_published: 0,
        }
    }

    /// Publish updated parameters to all cores via SeqLock.
    pub fn publish_parameters(&self, params: Parameters) {
        self.params.write(params);
    }

    /// Read current parameters.
    pub fn current_parameters(&self) -> Parameters {
        self.params.read()
    }

    /// Send a trade event to a specific core by index.
    pub fn send_event(&mut self, core_idx: usize, event: TradeEvent) -> Result<(), TradeEvent> {
        if core_idx >= self.producers.len() {
            return Err(event);
        }
        let result = self.producers[core_idx].try_push(event);
        if result.is_ok() {
            self.events_published += 1;
        }
        result
    }

    /// Route a trade event to the appropriate core based on market_id.
    pub fn route_event(&mut self, event: TradeEvent) -> Result<(), TradeEvent> {
        if self.producers.is_empty() {
            return Err(event);
        }
        let core_idx = event.market_id as usize % self.producers.len();
        self.send_event(core_idx, event)
    }

    /// Broadcast a trade event to all cores.
    pub fn broadcast_event(&mut self, event: TradeEvent) -> usize {
        let mut sent = 0;
        for producer in &self.producers {
            if producer.try_push(event).is_ok() {
                sent += 1;
            }
        }
        self.events_published += sent as u64;
        sent
    }

    /// Collect signals from external signal vecs.
    pub fn collect_signals(&mut self, signals: &mut Vec<Signal>) {
        self.collected_signals.append(signals);
    }

    /// Drain all collected signals.
    pub fn drain_collected_signals(&mut self) -> Vec<Signal> {
        std::mem::take(&mut self.collected_signals)
    }

    pub fn events_published(&self) -> u64 {
        self.events_published
    }

    pub fn num_cores(&self) -> usize {
        self.producers.len()
    }
}

/// Create ring buffers and shared parameter SeqLock for a multi-core setup.
pub fn create_infrastructure(
    num_cores: usize,
    buffer_capacity: usize,
) -> (Vec<SpscRingBuffer<TradeEvent>>, Arc<SeqLock<Parameters>>) {
    let buffers: Vec<_> = (0..num_cores)
        .map(|_| SpscRingBuffer::new(buffer_capacity))
        .collect();
    let params = Arc::new(SeqLock::new(Parameters::default()));
    (buffers, params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Side;

    fn make_event(market_id: u32) -> TradeEvent {
        TradeEvent {
            timestamp_ns: 1,
            price: 0.55,
            quantity: 10.0,
            side: Side::Buy,
            market_id,
        }
    }

    #[test]
    fn new_controller() {
        let (buffers, params) = create_infrastructure(2, 64);
        let producers: Vec<_> = buffers.iter().map(|b| b.split().0).collect();
        let ctrl = Controller::new(producers, params.clone());
        assert_eq!(ctrl.num_cores(), 2);
        assert_eq!(ctrl.events_published(), 0);
        let _ = params;
    }

    #[test]
    fn publish_parameters() {
        let (buffers, params) = create_infrastructure(1, 64);
        let producers: Vec<_> = buffers.iter().map(|b| b.split().0).collect();
        let ctrl = Controller::new(producers, params);

        let new_params = Parameters {
            max_position: 500.0,
            risk_limit: 0.1,
            spread_threshold: 0.005,
            momentum_window: 50,
        };
        ctrl.publish_parameters(new_params);
        let p = ctrl.current_parameters();
        assert_eq!(p.max_position, 500.0);
        assert_eq!(p.momentum_window, 50);
    }

    #[test]
    fn current_parameters_default() {
        let (buffers, params) = create_infrastructure(1, 64);
        let producers: Vec<_> = buffers.iter().map(|b| b.split().0).collect();
        let ctrl = Controller::new(producers, params);
        let p = ctrl.current_parameters();
        assert_eq!(p.max_position, 100.0);
    }

    #[test]
    fn send_event_valid() {
        let rb = SpscRingBuffer::<TradeEvent>::new(64);
        let (prod, cons) = rb.split();
        let params = Arc::new(SeqLock::new(Parameters::default()));
        let mut ctrl = Controller::new(vec![prod], params);

        assert!(ctrl.send_event(0, make_event(0)).is_ok());
        assert_eq!(ctrl.events_published(), 1);
        assert_eq!(cons.try_pop().unwrap().market_id, 0);
    }

    #[test]
    fn send_event_invalid_core() {
        let rb = SpscRingBuffer::<TradeEvent>::new(64);
        let (prod, _cons) = rb.split();
        let params = Arc::new(SeqLock::new(Parameters::default()));
        let mut ctrl = Controller::new(vec![prod], params);

        assert!(ctrl.send_event(5, make_event(0)).is_err());
        assert_eq!(ctrl.events_published(), 0);
    }

    #[test]
    fn route_event_basic() {
        let rb = SpscRingBuffer::<TradeEvent>::new(64);
        let (prod, cons) = rb.split();
        let params = Arc::new(SeqLock::new(Parameters::default()));
        let mut ctrl = Controller::new(vec![prod], params);

        ctrl.route_event(make_event(0)).unwrap();
        assert!(cons.try_pop().is_some());
    }

    #[test]
    fn route_event_round_robin() {
        let rb0 = SpscRingBuffer::<TradeEvent>::new(64);
        let rb1 = SpscRingBuffer::<TradeEvent>::new(64);
        let (prod0, cons0) = rb0.split();
        let (prod1, cons1) = rb1.split();
        let params = Arc::new(SeqLock::new(Parameters::default()));
        let mut ctrl = Controller::new(vec![prod0, prod1], params);

        // market_id 0 → core 0, market_id 1 → core 1
        ctrl.route_event(make_event(0)).unwrap();
        ctrl.route_event(make_event(1)).unwrap();
        ctrl.route_event(make_event(2)).unwrap(); // → core 0
        ctrl.route_event(make_event(3)).unwrap(); // → core 1

        assert_eq!(cons0.len(), 2);
        assert_eq!(cons1.len(), 2);
    }

    #[test]
    fn route_event_empty_producers() {
        let params = Arc::new(SeqLock::new(Parameters::default()));
        let mut ctrl = Controller::new(Vec::new(), params);
        assert!(ctrl.route_event(make_event(0)).is_err());
    }

    #[test]
    fn broadcast_event() {
        let rb0 = SpscRingBuffer::<TradeEvent>::new(64);
        let rb1 = SpscRingBuffer::<TradeEvent>::new(64);
        let (prod0, cons0) = rb0.split();
        let (prod1, cons1) = rb1.split();
        let params = Arc::new(SeqLock::new(Parameters::default()));
        let mut ctrl = Controller::new(vec![prod0, prod1], params);

        let sent = ctrl.broadcast_event(make_event(0));
        assert_eq!(sent, 2);
        assert!(cons0.try_pop().is_some());
        assert!(cons1.try_pop().is_some());
    }

    #[test]
    fn collect_signals() {
        let params = Arc::new(SeqLock::new(Parameters::default()));
        let mut ctrl: Controller = Controller::new(Vec::new(), params);

        let mut signals = vec![
            crate::types::Signal {
                timestamp_ns: 1,
                direction: crate::types::Direction::Long,
                strength: 0.5,
                market_id: 0,
            },
            crate::types::Signal {
                timestamp_ns: 2,
                direction: crate::types::Direction::Short,
                strength: 0.3,
                market_id: 1,
            },
        ];
        ctrl.collect_signals(&mut signals);
        assert!(signals.is_empty());
        assert_eq!(ctrl.drain_collected_signals().len(), 2);
    }

    #[test]
    fn drain_collected_signals_empties() {
        let params = Arc::new(SeqLock::new(Parameters::default()));
        let mut ctrl: Controller = Controller::new(Vec::new(), params);
        let drained = ctrl.drain_collected_signals();
        assert!(drained.is_empty());
    }

    #[test]
    fn events_published_count() {
        let rb = SpscRingBuffer::<TradeEvent>::new(64);
        let (prod, _cons) = rb.split();
        let params = Arc::new(SeqLock::new(Parameters::default()));
        let mut ctrl = Controller::new(vec![prod], params);

        for i in 0..10 {
            ctrl.send_event(0, make_event(i)).unwrap();
        }
        assert_eq!(ctrl.events_published(), 10);
    }

    #[test]
    fn num_cores() {
        let (buffers, params) = create_infrastructure(4, 32);
        let producers: Vec<_> = buffers.iter().map(|b| b.split().0).collect();
        let ctrl = Controller::new(producers, params);
        assert_eq!(ctrl.num_cores(), 4);
    }

    #[test]
    fn create_infrastructure_sizes() {
        let (buffers, _params) = create_infrastructure(3, 128);
        assert_eq!(buffers.len(), 3);
        assert_eq!(buffers[0].capacity(), 128);
    }

    #[test]
    fn full_cycle_integration() {
        let rb = SpscRingBuffer::<TradeEvent>::new(64);
        let (prod, cons) = rb.split();
        let params = Arc::new(SeqLock::new(Parameters {
            max_position: 1000.0,
            risk_limit: 1.0,
            spread_threshold: 0.0,
            momentum_window: 2,
        }));
        let mut ctrl = Controller::new(vec![prod], params.clone());

        // Send events through controller
        for i in 0..5 {
            let event = TradeEvent {
                timestamp_ns: i,
                price: 1.00 + (i as f64) * 0.01,
                quantity: 10.0,
                side: Side::Buy,
                market_id: 0,
            };
            ctrl.send_event(0, event).unwrap();
        }

        // Create engine and process
        let mut engine = crate::core_engine::CoreEngine::new(0, 0, cons, params);
        let processed = engine.process_tick();
        assert_eq!(processed, 5);

        // Drain signals
        let mut signals = Vec::new();
        engine.drain_signals(&mut signals);
        ctrl.collect_signals(&mut signals);

        assert_eq!(ctrl.events_published(), 5);
    }
}
