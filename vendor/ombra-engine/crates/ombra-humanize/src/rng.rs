//! Random numbers for the persona and the planners.
//!
//! `SplitMix64` seeds everything and drives the persona expansions (decision D5); `Rng`
//! (xoshiro256++) samples. Both algorithms are pinned by tests against published vectors:
//! changing either changes every persona derived from a seed.

/// SplitMix64 (Vigna).
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// FNV-1a, 64-bit: names a parameter's own random stream.
pub fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3))
}

fn unit_from(bits: u64) -> f64 {
    (bits >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
}

/// Box–Muller from two uniforms in [0, 1); `1 - u1` keeps the logarithm finite.
fn box_muller(u1: f64, u2: f64) -> f64 {
    (-2.0 * (1.0 - u1).ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Standard normals for the persona expansions (D5): SplitMix64 + Box–Muller.
pub fn normal_stream(seed: u64) -> impl Iterator<Item = f64> {
    let mut bits = SplitMix64::new(seed);
    std::iter::from_fn(move || Some(box_muller(unit_from(bits.next_u64()), unit_from(bits.next_u64()))))
}

/// xoshiro256++ (Blackman, Vigna), seeded through SplitMix64.
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut bits = SplitMix64::new(seed);
        Rng { s: [bits.next_u64(), bits.next_u64(), bits.next_u64(), bits.next_u64()] }
    }

    /// The stream of one named parameter of one seed. Adding a parameter never moves the others.
    pub fn for_label(seed: u64, label: &str) -> Self {
        Self::new(seed ^ fnv1a(label))
    }

    /// For tests against the reference implementation.
    pub fn from_state(s: [u64; 4]) -> Self {
        Rng { s }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[0].wrapping_add(self.s[3]).rotate_left(23).wrapping_add(self.s[0]);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        unit_from(self.next_u64())
    }

    pub fn uniform(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    /// Uniform index in 0..n (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        debug_assert!(n > 0, "below(0)");
        ((self.unit() * n as f64) as usize).min(n - 1)
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub fn normal(&mut self) -> f64 {
        box_muller(self.unit(), self.unit())
    }

    pub fn lognormal(&mut self, median: f64, sigma: f64) -> f64 {
        median * (sigma * self.normal()).exp()
    }

    /// Poisson(lambda) by Knuth's product of uniforms; lambda is small here (stops in a movement).
    pub fn poisson(&mut self, lambda: f64) -> u32 {
        let limit = (-lambda.max(0.0)).exp();
        let (mut k, mut product) = (0u32, self.unit());
        while product > limit && k < 64 {
            k += 1;
            product *= self.unit();
        }
        k
    }

    pub fn log_uniform(&mut self, low: f64, high: f64) -> f64 {
        self.uniform(low.ln(), high.ln()).exp()
    }

    /// Gamma(shape, 1): Marsaglia–Tsang, with the boost for shape < 1.
    pub fn gamma(&mut self, shape: f64) -> f64 {
        if shape < 1.0 {
            let u = 1.0 - self.unit();
            return self.gamma(shape + 1.0) * u.powf(1.0 / shape);
        }
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        loop {
            let x = self.normal();
            let v = 1.0 + c * x;
            if v <= 0.0 {
                continue;
            }
            let v = v * v * v;
            let u = 1.0 - self.unit();
            if u.ln() < 0.5 * x * x + d - d * v + d * v.ln() {
                return d * v;
            }
        }
    }

    pub fn beta(&mut self, a: f64, b: f64) -> f64 {
        let x = self.gamma(a);
        let y = self.gamma(b);
        x / (x + y)
    }

    /// Redraws `sample` until it lands in [low, high]: a truncated distribution, not a clamp that
    /// would pile values on the bounds. Gives up after 1000 misses and clamps.
    pub fn truncated(&mut self, low: f64, high: f64, mut sample: impl FnMut(&mut Rng) -> f64) -> f64 {
        for _ in 0..1000 {
            let value = sample(self);
            if (low..=high).contains(&value) {
                return value;
            }
        }
        sample(self).clamp(low, high)
    }
}
