//! What the music does, as it happens. A filter bank instead of an FFT: a filter answers within
//! a few milliseconds where a window long enough for the bass needs tens, and there are no bars
//! to draw here. Strength is measured against the song's own recent peaks, slowly forgotten, so
//! a quiet verse stays quiet next to its chorus (cava's autosens evens them out).

pub const RATE: f32 = 48000.0;
const HOP: usize = 96; // 2 ms
const HOP_S: f32 = HOP as f32 / RATE;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Kick,
    Snare,
    Hat,
    Beat,
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub kind: Kind,
    pub strength: f32,
    pub at: f64, // seconds of audio heard
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Frame {
    pub bass: f32, // 0..1 against the song's own peaks
    pub loud: f32,
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn new(freq: f32, high: bool) -> Self {
        let w = std::f32::consts::TAU * freq / RATE;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let n = if high { (1.0 + c) / 2.0 } else { (1.0 - c) / 2.0 };
        let m = if high { -(1.0 + c) } else { 1.0 - c };
        Biquad { b: [n / a0, m / a0, n / a0], a: [-2.0 * c / a0, (1.0 - alpha) / a0], z: [0.0; 2] }
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

fn db(e: f32) -> f32 {
    10.0 * (e + 1e-12).log10()
}

/// One band: its energy over a short window against the window just before it, so a sustained
/// note reads flat (the window spans the low band's ripple) while a hit jumps.
struct Band {
    hp: Biquad,
    lp: Biquad,
    sum: f32,
    hist: Vec<f32>, // the last 2 * win hops
    at: usize,
    win: usize,
    now: f32,
    peak: f32, // dB, the song's recent loudest, slowly forgotten
    hit_peak: f32, // dB, the same of the level at the moment hits were heard
    recent: f32, // dB, the loudest of the last seconds: what the gates compare with
    lift: f32, // how much energy this window gained on the last, as a share of `recent`
}

impl Band {
    fn new(lo: f32, hi: f32, win_ms: f32) -> Self {
        let win = ((win_ms / 1000.0 / HOP_S).round() as usize).max(1);
        Band { hp: Biquad::new(lo, true), lp: Biquad::new(hi, false), sum: 0.0, hist: vec![0.0; win * 2], at: 0, win, now: 0.0, peak: -90.0, hit_peak: -90.0, recent: -90.0, lift: 0.0 }
    }
    #[inline]
    fn feed(&mut self, x: f32) -> f32 {
        let y = self.lp.run(self.hp.run(x));
        self.sum += y * y;
        y
    }
    /// Ends the hop. Returns the rise in dB.
    fn hop(&mut self) -> f32 {
        self.hist[self.at] = self.sum / HOP as f32;
        self.sum = 0.0;
        self.at = (self.at + 1) % self.hist.len();
        let n = self.hist.len();
        let (mut new, mut old) = (0.0, 0.0);
        for i in 0..self.win {
            new += self.hist[(self.at + n - 1 - i) % n];
            old += self.hist[(self.at + n - 1 - i - self.win) % n];
        }
        self.now = db(new / self.win as f32);
        self.peak = if self.now > self.peak { self.now } else { self.peak - 0.15 * HOP_S };
        self.hit_peak -= 0.15 * HOP_S;
        self.recent = if self.now > self.recent { self.now } else { self.recent - 2.0 * HOP_S };
        let unit = 10f32.powf(self.recent / 10.0);
        self.lift = (new - old) / self.win as f32 / unit.max(1e-12);
        // silence is 40 dB under the peak at the lowest: out of it every hit would be a huge jump
        self.now - db(old / self.win as f32).max(self.peak - 40.0)
    }
    fn below_peak(&self) -> f32 {
        self.peak - self.now
    }
    fn below_recent(&self) -> f32 {
        self.recent - self.now
    }
    /// 0..1: half this hit against the song's recent hits, so it follows the song's sections,
    /// half against full scale, so the first quiet verse isn't full strength for having no past
    fn hit(&mut self, range: f32, floor: f32) -> f32 {
        self.hit_at(self.now, range, floor)
    }
    fn hit_at(&mut self, level: f32, range: f32, floor: f32) -> f32 {
        self.hit_peak = self.hit_peak.max(level);
        let rel = 1.0 - (self.hit_peak - level) / range;
        let abs = (level - floor) / -floor;
        (0.5 * rel.clamp(0.0, 1.0) + 0.5 * abs.clamp(0.0, 1.0)).clamp(0.0, 1.0)
    }
}

/// The tempo from the gaps between kicks: their median, folded into 70..180 bpm.
#[derive(Default)]
struct Tempo {
    gaps: Vec<f64>,
    period: f64,
    next: f64,
    last_kick: f64,
    missed: u32,
}

impl Tempo {
    fn kick(&mut self, t: f64) {
        let gap = t - self.last_kick;
        if self.last_kick > 0.0 && gap < 2.0 {
            let mut g = gap;
            while g > 60.0 / 70.0 {
                g /= 2.0;
            }
            while g < 60.0 / 180.0 {
                g *= 2.0;
            }
            self.gaps.push(g);
            if self.gaps.len() > 8 {
                self.gaps.remove(0);
            }
            if self.gaps.len() >= 3 {
                let mut s = self.gaps.clone();
                s.sort_by(f64::total_cmp);
                self.period = s[s.len() / 2];
            }
        }
        self.last_kick = t;
        if self.period > 0.0 {
            let d = t - (self.next - self.period);
            if self.next == 0.0 || d.abs() > self.period * 0.5 {
                self.next = t + self.period;
            } else if d.abs() < self.period * 0.25 {
                self.next -= d * 0.5;
            }
            self.missed = 0;
        }
    }
    /// How near a beat `t` falls, as a share of the period (0 = on it). None with no tempo.
    fn off_grid(&self, t: f64) -> Option<f64> {
        if self.period <= 0.0 || self.gaps.len() < 4 {
            return None;
        }
        let d = (self.next - t) / (self.period / 2.0); // half-beats count: offbeat kicks are common
        Some((d - d.round()).abs() / 2.0)
    }
    fn tick(&mut self, t: f64) -> bool {
        if self.period > 0.0 && self.next > 0.0 && t >= self.next {
            self.next += self.period;
            if t - self.last_kick > self.period * 1.5 {
                self.missed += 1;
                if self.missed > 16 {
                    *self = Tempo::default();
                }
            }
            return true;
        }
        false
    }
}

// Tuned on tests/synth.rs and on real music; what each is for is said where it is used.
const KICK_JUMP: f32 = 0.4; // of the song's recent kicks' amplitude, within ENV_HOPS
const KICK_JUMP_ON_GRID: f32 = 0.25;
const KICK_MIX: f32 = 22.0; // dB under the whole sound's peak: quieter is no drum
const KICK_GAP: f64 = 0.13;
const KICK_TOP: f64 = 0.006; // s at most to the top of the jump
const ENV_HOPS: usize = 3; // 6 ms
const LOW_REF_FORGET: f32 = 0.999655; // 1.5 dB a second: a verse after a chorus is heard soon
const LOW_ENV_FALL: f32 = 0.99965; // per sample, 60 ms: spans the ripple of a 40 Hz note

const SNARE_RISE: f32 = 3.0;
const SNARE_TOP_RISE: f32 = 5.0;
const SNARE_GATE: f32 = 8.0;
const SNARE_TOP_GATE: f32 = 5.0; // the wires' noise fills the very top: a kick's click or a voice doesn't
const SNARE_MIX: f32 = 22.0;
const SNARE_GAP: f64 = 0.12;
const HAT_RISE: f32 = 6.0;
const HAT_GATE: f32 = 18.0;
const HAT_MIX: f32 = 42.0;
const HAT_LIFT: f32 = 0.25; // of the top's recent peak, within a window: a slow "s" out of silence has the rise, not this
const HAT_GAP: f64 = 0.06;
const HAT_OVER_S: f32 = 10.0;
const HAT_WAIT: f64 = 0.006;

pub struct Ear {
    low: Band,
    mid: Band,
    top: Band,
    sib: Band,
    all: Band,
    n: usize,
    hops: u64,
    last_kick: f64,
    low_env: f32,
    low_ref: f32,
    env_hist: [f32; ENV_HOPS],
    env_at: usize,
    low_jumped: Option<(f64, f32)>,
    last_snare: f64,
    last_hat: f64,
    hat_pending: Option<f64>,
    tempo: Tempo,
    bass: f32,
}

impl Default for Ear {
    fn default() -> Self {
        Ear {
            low: Band::new(35.0, 130.0, 18.0),
            mid: Band::new(500.0, 3000.0, 10.0),
            top: Band::new(9000.0, 16000.0, 6.0),
            sib: Band::new(4000.0, 7500.0, 6.0),
            all: Band::new(30.0, 16000.0, 30.0),
            n: 0,
            hops: 0,
            last_kick: -1.0,
            low_env: 0.0,
            low_ref: 0.0,
            env_hist: [0.0; ENV_HOPS],
            env_at: 0,
            low_jumped: None,
            last_snare: -1.0,
            last_hat: -1.0,
            hat_pending: None,
            tempo: Tempo::default(),
            bass: 0.0,
        }
    }
}

impl Ear {
    /// Feeds samples (mono, 48 kHz); events land in `out` in order.
    pub fn feed(&mut self, samples: &[f32], out: &mut Vec<Event>) {
        for &x in samples {
            let y = self.low.feed(x);
            self.low_env = (self.low_env * LOW_ENV_FALL).max(y.abs());
            self.mid.feed(x);
            self.top.feed(x);
            self.sib.feed(x);
            self.all.feed(x);
            self.n += 1;
            if self.n == HOP {
                self.n = 0;
                self.hop(out);
            }
        }
    }

    pub fn frame(&self) -> Frame {
        Frame { bass: self.bass, loud: (1.0 - self.all.below_peak() / 30.0).clamp(0.0, 1.0) }
    }

    fn hop(&mut self, out: &mut Vec<Event>) {
        self.hops += 1;
        let t = self.hops as f64 * HOP_S as f64;
        self.low.hop();
        let mid = self.mid.hop();
        let top = self.top.hop();
        self.sib.hop();
        self.all.hop();
        let b = (1.0 - self.low.below_peak() / 24.0).clamp(0.0, 1.0);
        self.bass = if b > self.bass { b } else { self.bass + (b - self.bass) * (1.0 - (-HOP_S / 0.12f32).exp()) };

        // The kick: the low band's amplitude jumps by a good part of the song's recent hits within
        // a few ms. A bass line moves it less, already being up; a soft bass note takes longer. On
        // the song's beat a smaller jump counts, catching the kick under a busy bar. Heard at the
        // top of the jump, a few ms later, so its strength is the hit's own.
        let mix = self.all.peak;
        self.low_ref = (self.low_ref * LOW_REF_FORGET).max(self.low_env);
        let jump = self.low_env - self.env_hist[self.env_at];
        self.env_hist[self.env_at] = self.low_env;
        self.env_at = (self.env_at + 1) % self.env_hist.len();
        let on_grid = self.tempo.off_grid(t).is_some_and(|d| d < 0.06);
        let need = if on_grid { KICK_JUMP_ON_GRID } else { KICK_JUMP } * self.low_ref;
        let ready = t - self.last_kick > KICK_GAP && mix - db(self.low_env * self.low_env) < KICK_MIX;
        match self.low_jumped {
            None if jump > need && ready => self.low_jumped = Some((t, self.low_env)),
            Some((since, was)) if self.low_env <= was || t - since >= KICK_TOP => {
                self.low_jumped = None;
                self.last_kick = t;
                self.tempo.kick(t);
                let strength = self.low.hit_at(db(self.low_env * self.low_env), 14.0, -24.0);
                out.push(Event { kind: Kind::Kick, strength, at: t });
            }
            Some((since, _)) => self.low_jumped = Some((since, self.low_env)),
            _ => {}
        }

        // The snare: mids and the very top jump together (the wires' noise); a voice or a piano
        // moves the mids alone.
        let snare = mid > SNARE_RISE && top > SNARE_TOP_RISE && self.mid.below_recent() < SNARE_GATE && self.top.below_recent() < SNARE_TOP_GATE && mix - self.mid.now < SNARE_MIX && t - self.last_snare > SNARE_GAP;
        if snare {
            self.last_snare = t;
            out.push(Event { kind: Kind::Snare, strength: self.mid.hit(12.0, -40.0), at: t });
        }

        // The hat: the very top jumps and holds its own against the band below it, where a voice's
        // "s" sits (a jump measures the same in both; the level tells them apart). Said a moment
        // later: the top of a snare gets there before its mids do, and then it was the snare.
        if snare {
            self.hat_pending = None;
        }
        if !snare && t - self.last_snare > HAT_WAIT && top > HAT_RISE && self.top.lift > HAT_LIFT && self.top.now > self.sib.now - HAT_OVER_S && self.top.below_recent() < HAT_GATE && mix - self.top.now < HAT_MIX && t - self.last_hat > HAT_GAP {
            self.last_hat = t;
            self.hat_pending = Some(t);
        }
        if self.hat_pending.is_some_and(|at| t - at >= HAT_WAIT) {
            self.hat_pending = None;
            out.push(Event { kind: Kind::Hat, strength: self.top.hit(14.0, -60.0), at: t });
        }

        if self.tempo.tick(t) {
            out.push(Event { kind: Kind::Beat, strength: 1.0, at: t });
        }
    }
}
