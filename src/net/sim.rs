//! A simulated network and computers for the sync trial: links with latency,
//! jitter, spikes and loss, clocks with an offset and a drift, and a queue of
//! events in true time (the simulator's own). Deterministic: everything random
//! comes from a seeded xorshift.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// xorshift64* (Vigna): small, fast, and good enough for jitter.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        // Never zero, and seeds that differ a little start far apart.
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for _ in 0..4 {
            r.next_u64();
        }
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    /// Exponential with this mean.
    pub fn exp(&mut self, mean: f64) -> f64 {
        -mean * (1.0 - self.unit()).ln()
    }
}

/// How a one-way delay varies around its base.
#[derive(Clone, Copy, Debug)]
pub enum Jitter {
    /// Triangular, ± this (a wired switch's small queues).
    Even(f64),
    /// Never below the base, with an exponential tail of this mean above it
    /// (radio: a floor of air time and retries above it).
    Tail(f64),
}

/// One direction of a path.
#[derive(Clone, Copy, Debug)]
pub struct Link {
    /// Seconds.
    pub base: f64,
    pub jitter: Jitter,
    /// The chance a packet is held up by `spike` more (seconds).
    pub spike_chance: f64,
    pub spike: f64,
    pub loss: f64,
}

impl Link {
    /// How long a packet takes, or None if it is lost.
    pub fn delay(&self, rng: &mut Rng) -> Option<f64> {
        if rng.chance(self.loss) {
            return None;
        }
        let d = match self.jitter {
            Jitter::Even(j) => self.base + j * (rng.unit() + rng.unit() - 1.0),
            Jitter::Tail(mean) => self.base + rng.exp(mean),
        };
        let spike = if rng.chance(self.spike_chance) {
            self.spike * rng.range(0.5, 1.5)
        } else {
            0.0
        };
        Some(d.max(20e-6) + spike)
    }
}

/// A network between the primary and a follower: each way.
#[derive(Clone, Debug)]
pub struct Network {
    pub name: &'static str,
    /// Follower to primary, and back.
    pub up: Link,
    pub down: Link,
}

impl Network {
    /// A show LAN: 1 ms each way, ±0.3 ms, 25 µs of asymmetry.
    pub fn wired() -> Network {
        let link = |base| Link {
            base,
            jitter: Jitter::Even(0.000_3),
            spike_chance: 0.0,
            spike: 0.0,
            loss: 0.0,
        };
        Network {
            name: "Wired show LAN (1 ms, ±0.3 ms)",
            up: link(0.001_025),
            down: link(0.000_975),
        }
    }

    /// A poorer wired one: 5 ms, ±2 ms, 0.25 ms of asymmetry, rare 20 ms holds.
    pub fn poor_wired() -> Network {
        let link = |base| Link {
            base,
            jitter: Jitter::Even(0.002),
            spike_chance: 0.002,
            spike: 0.020,
            loss: 0.001,
        };
        Network {
            name: "Poorer wired (5 ms, ±2 ms)",
            up: link(0.005_25),
            down: link(0.004_75),
        }
    }

    /// Wi-Fi: about 8 ms on average (a 2 ms floor and a tail, ±15 ms around
    /// it), 1 % of packets held up 100 ms, 2 % lost, the way up 1 ms slower.
    pub fn wifi() -> Network {
        let link = |base| Link {
            base,
            jitter: Jitter::Tail(0.006),
            spike_chance: 0.01,
            spike: 0.100,
            loss: 0.02,
        };
        Network {
            name: "Wi-Fi (8 ms, ±15 ms, 100 ms spikes, 2 % loss)",
            up: link(0.002_5),
            down: link(0.001_5),
        }
    }

    pub fn all() -> Vec<Network> {
        vec![Network::wired(), Network::poor_wired(), Network::wifi()]
    }
}

/// A computer's own monotonic clock against true time.
#[derive(Clone, Copy, Debug)]
pub struct NodeClock {
    /// Its reading at true time 0.
    pub offset: f64,
    /// How fast it runs, in parts per million.
    pub ppm: f64,
}

impl NodeClock {
    pub fn local(&self, true_t: f64) -> f64 {
        self.offset + true_t * (1.0 + self.ppm * 1e-6)
    }

    pub fn true_at(&self, local: f64) -> f64 {
        (local - self.offset) / (1.0 + self.ppm * 1e-6)
    }
}

/// Events in true time, in order, ties in the order they were put in.
pub struct Queue<T> {
    heap: BinaryHeap<Entry<T>>,
    seq: u64,
}

struct Entry<T> {
    at: f64,
    seq: u64,
    what: T,
}

impl<T> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl<T> Eq for Entry<T> {}
impl<T> PartialOrd for Entry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Entry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed: the heap is a max-heap, we want the earliest.
        other
            .at
            .total_cmp(&self.at)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Queue {
            heap: BinaryHeap::new(),
            seq: 0,
        }
    }
}

impl<T> Queue<T> {
    pub fn push(&mut self, at: f64, what: T) {
        self.seq += 1;
        self.heap.push(Entry {
            at,
            seq: self.seq,
            what,
        });
    }

    pub fn pop(&mut self) -> Option<(f64, T)> {
        self.heap.pop().map(|e| (e.at, e.what))
    }

    /// When the next event is.
    pub fn peek_time(&self) -> Option<f64> {
        self.heap.peek().map(|e| e.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_numbers() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut c = Rng::new(8);
        assert_ne!(Rng::new(7).next_u64(), c.next_u64());
        let mean = (0..100_000).map(|_| c.unit()).sum::<f64>() / 100_000.0;
        assert!((mean - 0.5).abs() < 0.01, "{mean}");
    }

    #[test]
    fn links_are_as_described() {
        let mut rng = Rng::new(1);
        let n = 100_000;
        for net in Network::all() {
            let mut sum = 0.0;
            let mut lost = 0;
            let mut least = f64::INFINITY;
            for _ in 0..n {
                match net.down.delay(&mut rng) {
                    Some(d) => {
                        sum += d;
                        least = least.min(d);
                    }
                    None => lost += 1,
                }
            }
            let mean = sum / (n - lost) as f64;
            let expect = match net.down.jitter {
                Jitter::Even(_) => net.down.base,
                Jitter::Tail(m) => net.down.base + m,
            } + net.down.spike_chance * net.down.spike;
            assert!(
                (mean - expect).abs() < expect * 0.05,
                "{}: {mean}",
                net.name
            );
            assert!(least >= 20e-6);
            let loss = lost as f64 / n as f64;
            assert!((loss - net.down.loss).abs() < 0.005, "{}: {loss}", net.name);
        }
    }

    #[test]
    fn a_clock_and_its_inverse_agree_and_the_queue_keeps_order() {
        let c = NodeClock {
            offset: 3.5,
            ppm: -50.0,
        };
        assert!((c.true_at(c.local(1234.5)) - 1234.5).abs() < 1e-9);
        let mut q = Queue::default();
        q.push(2.0, "c");
        q.push(1.0, "a");
        q.push(1.0, "b");
        assert_eq!(q.pop(), Some((1.0, "a")));
        assert_eq!(q.pop(), Some((1.0, "b")));
        assert_eq!(q.pop(), Some((2.0, "c")));
        assert_eq!(q.pop(), None);
    }
}
