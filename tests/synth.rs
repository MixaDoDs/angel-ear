//! A made-up song with known hits: does the ear find them, on time, and nothing else?
use angel_ear::{Ear, Kind, RATE};

struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

fn add(track: &mut [f32], at: f64, gain: f32, len: f64, f: &mut impl FnMut(f32) -> f32) {
    let start = (at * RATE as f64) as usize;
    for i in 0..(len * RATE as f64) as usize {
        if let Some(s) = track.get_mut(start + i) {
            *s += gain * f(i as f32 / RATE);
        }
    }
}

fn kick(track: &mut [f32], at: f64, g: f32) {
    let mut ph = 0.0f32;
    add(track, at, g, 0.18, &mut |t| {
        ph += std::f32::consts::TAU * (50.0 + 70.0 * (-t / 0.03).exp()) / RATE;
        ph.sin() * (-t / 0.05).exp() * (1.0 - t / 0.18)
    });
}
fn snare(track: &mut [f32], at: f64, g: f32, n: &mut Noise) {
    add(track, at, g, 0.15, &mut |t| (n.next() * 0.8 + (t * 1200.0).sin() * 0.3) * (-t / 0.04).exp() * (1.0 - t / 0.15));
}
fn hat(track: &mut [f32], at: f64, g: f32, n: &mut Noise) {
    let mut prev = 0.0;
    add(track, at, g, 0.04, &mut |t| {
        let x = n.next();
        let hp = x - prev;
        prev = x;
        hp * (-t / 0.01).exp() * (1.0 - t / 0.04)
    });
}
/// A bass line: long notes, soft attack, off the kick
fn bass(track: &mut [f32], at: f64, g: f32, hz: f32) {
    add(track, at, g, 0.4, &mut |t| (std::f32::consts::TAU * hz * t).sin() * (t / 0.025).min(1.0) * (1.0 - t / 0.4));
}
/// A voice: harmonics in the mids that come and go, and an "s" in the 4–7 kHz band
fn voice(track: &mut [f32], at: f64, g: f32, n: &mut Noise) {
    add(track, at, g, 0.35, &mut |t| {
        (1..7).map(|h| (std::f32::consts::TAU * 220.0 * h as f32 * t).sin() / h as f32).sum::<f32>() * (t / 0.02).min(1.0) * (1.0 - t / 0.35)
    });
    let mut z = [0.0f32; 2];
    add(track, at + 0.3, g * 0.5, 0.14, &mut |t| {
        let x = n.next();
        let y = x + 1.28 * z[0] - 0.72 * z[1]; // a resonance near 5.5 kHz
        z = [y, z[0]];
        y * 0.3 * (t / 0.03).min(1.0) * (1.0 - t / 0.14)
    });
}

struct Truth {
    track: Vec<f32>,
    kicks: Vec<f64>,
    snares: Vec<f64>,
    hats: Vec<f64>,
}

/// 124 bpm, 32 bars: a quiet verse, then a chorus 12 dB louder
fn song() -> Truth {
    let beat = 60.0 / 124.0;
    let bars = 32;
    let mut t = Truth { track: vec![0.0; ((bars * 4 + 1) as f64 * beat * RATE as f64) as usize], kicks: vec![], snares: vec![], hats: vec![] };
    let mut n = Noise(0x1234567);
    for b in 0..bars * 4 {
        let at = b as f64 * beat;
        let g = if b < bars * 2 { 0.25 } else { 1.0 };
        if b % 2 == 0 {
            kick(&mut t.track, at, 0.7 * g);
            t.kicks.push(at);
        } else {
            kick(&mut t.track, at, 0.5 * g);
            t.kicks.push(at);
            snare(&mut t.track, at, 0.5 * g, &mut n);
            t.snares.push(at);
        }
        hat(&mut t.track, at + beat / 2.0, 0.4 * g, &mut n);
        t.hats.push(at + beat / 2.0);
        bass(&mut t.track, at + beat * 0.25, 0.35 * g, 55.0);
        if b % 3 == 0 {
            voice(&mut t.track, at + beat * 0.3, 0.2 * g, &mut n);
        }
    }
    t
}

/// (found, false, the mean delay in ms)
fn score(truth: &[f64], heard: &[f64], window: f64) -> (usize, usize, f64) {
    let (mut found, mut delay) = (0, 0.0);
    for &x in truth {
        if let Some(h) = heard.iter().find(|&&h| h >= x - 0.002 && h <= x + window) {
            found += 1;
            delay += h - x;
        }
    }
    let extra = heard.iter().filter(|&&h| !truth.iter().any(|&x| h >= x - 0.002 && h <= x + window)).count();
    (found, extra, delay / found.max(1) as f64 * 1000.0)
}

#[test]
fn hears_the_song() {
    let s = song();
    let (mut ear, mut ev) = (Ear::default(), vec![]);
    ear.feed(&s.track, &mut ev);
    let times = |k: Kind| ev.iter().filter(|e| e.kind == k).map(|e| e.at).collect::<Vec<_>>();
    let (kf, ke, kd) = score(&s.kicks, &times(Kind::Kick), 0.05);
    let (sf, se, sd) = score(&s.snares, &times(Kind::Snare), 0.05);
    let (hf, he, hd) = score(&s.hats, &times(Kind::Hat), 0.05);
    let beat = 60.0 / 124.0;
    let phases = |k: Kind, tr: &[f64]| ev.iter().filter(|e| e.kind == k && !tr.iter().any(|&x| e.at >= x - 0.002 && e.at <= x + 0.05)).map(|e| ((e.at % beat) / beat * 100.0) as u32).take(14).collect::<Vec<_>>();
    println!("extra kick phases % {:?} hat % {:?}", phases(Kind::Kick, &s.kicks), phases(Kind::Hat, &s.hats));
    if let Ok(p) = std::env::var("EAR_DUMP") {
        std::fs::write(p, s.track.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
    }
    println!("kicks {kf}/{} extra {ke} {kd:.1} ms; snares {sf}/{} extra {se} {sd:.1} ms; hats {hf}/{} extra {he} {hd:.1} ms", s.kicks.len(), s.snares.len(), s.hats.len());
    assert!(kf as f32 >= s.kicks.len() as f32 * 0.97 && ke <= 2 && kd < 25.0);
    assert!(sf as f32 >= s.snares.len() as f32 * 0.95 && se <= 2 && sd < 15.0);
    assert!(hf as f32 >= s.hats.len() as f32 * 0.95 && he <= 4 && hd < 15.0);
}

#[test]
fn a_chorus_hits_harder_than_a_verse() {
    let s = song();
    let (mut ear, mut ev) = (Ear::default(), vec![]);
    ear.feed(&s.track, &mut ev);
    let half = s.kicks[s.kicks.len() / 2];
    let mean = |it: Vec<f32>| it.iter().sum::<f32>() / it.len().max(1) as f32;
    let kicks = ev.iter().filter(|e| e.kind == Kind::Kick);
    let verse = mean(kicks.clone().filter(|e| e.at < half && e.at > half - 15.0).map(|e| e.strength).collect());
    let chorus = mean(kicks.filter(|e| e.at > half && e.at < half + 15.0).map(|e| e.strength).collect());
    println!("verse {verse:.2} chorus {chorus:.2}");
    assert!(chorus > verse + 0.15);
}

#[test]
fn a_voice_and_a_bass_line_are_no_drums() {
    let mut track = vec![0.0; (20.0 * RATE) as usize];
    let mut n = Noise(99);
    for i in 0..30 {
        voice(&mut track, i as f64 * 0.6, 0.4, &mut n);
        bass(&mut track, i as f64 * 0.6 + 0.1, 0.5, 55.0 + (i % 5) as f32 * 8.0);
    }
    let (mut ear, mut ev) = (Ear::default(), vec![]);
    ear.feed(&track, &mut ev);
    let count = |k: Kind| ev.iter().filter(|e| e.kind == k && e.at > 2.0).count();
    println!("kicks {} snares {} hats {}", count(Kind::Kick), count(Kind::Snare), count(Kind::Hat));
    assert!(count(Kind::Snare) <= 1 && count(Kind::Hat) <= 3 && count(Kind::Kick) <= 3);
}

