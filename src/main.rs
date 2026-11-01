//! angel-ear            listens to the program playing and prints what it hears, one per line:
//!                      K|S|H <strength 0..1>, B (a beat), F <bass> <loud> (30 a second)
//! angel-ear --file F   the same from raw mono f32 at 48 kHz, with the time first (for tests)
//! angel-ear --version
use angel_ear::{Ear, Event, Kind};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const NOT_MUSIC: &[&str] = &["discord", "vesktop", "telegram", "freshgram", "signal", "element", "slack", "zoom", "teams"];
const OWN: &[&str] = &["quickshell", "pw-play", "pw-cat", "angelos-sfx"]; // as Audio.isAppStream

extern "C" {
    fn prctl(option: i32, arg: u64, ...) -> i32;
}
fn die_with_parent() {
    unsafe { prctl(1, 9) }; // PR_SET_PDEATHSIG, SIGKILL
}

fn letter(k: Kind) -> char {
    match k {
        Kind::Kick => 'K',
        Kind::Snare => 'S',
        Kind::Hat => 'H',
        Kind::Beat => 'B',
    }
}

/// The streams worth listening to, from `pactl list sink-inputs`: playing, a program's, not a call.
fn streams_in(text: &str) -> Vec<u32> {
    let mut out = vec![];
    for block in text.split("Sink Input #").skip(1) {
        let Some(id) = block.lines().next().and_then(|l| l.trim().parse().ok()) else { continue };
        let prop = |k: &str| block.lines().find_map(|l| l.trim().strip_prefix(k)?.strip_prefix(" = \"")?.strip_suffix('"')).unwrap_or("");
        let app = if prop("application.name").is_empty() { prop("node.name") } else { prop("application.name") };
        let low = app.to_lowercase();
        if block.contains("Corked: yes")
            || prop("node.virtual") == "true"
            || prop("node.link-group").starts_with("loopback")
            || OWN.contains(&app)
            || NOT_MUSIC.iter().any(|n| low.contains(n))
            || prop("media.name").contains("/angelos/sounds/")
        {
            continue;
        }
        out.push(id);
    }
    out
}

fn scan() -> Vec<u32> {
    Command::new("pactl").args(["list", "sink-inputs"]).env("LC_ALL", "C").output().map(|o| streams_in(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

fn listen(id: u32, tx: mpsc::Sender<(u32, Vec<f32>)>) -> Option<Child> {
    let mut cmd = Command::new("parec");
    cmd.arg(format!("--monitor-stream={id}"))
        .args(["--rate=48000", "--channels=1", "--format=float32le", "--latency-msec=10", "--client-name=angelos-ear", "--property=node.name=angelos-ear"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    unsafe { cmd.pre_exec(|| { die_with_parent(); Ok(()) }) };
    let mut child = cmd.spawn().ok()?;
    let mut pipe = child.stdout.take()?;
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut have = 0;
        loop {
            match pipe.read(&mut buf[have..]) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    have += n;
                    let whole = have / 4 * 4;
                    let s = buf[..whole].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
                    buf.copy_within(whole..have, 0);
                    have -= whole;
                    if tx.send((id, s)).is_err() {
                        return;
                    }
                }
            }
        }
    });
    Some(child)
}

/// Follows one stream, the music: the loudest, and it stays until it falls silent or another is
/// much louder. Summing them would need aligning them; two programs at once are rare.
#[derive(Default)]
struct Pick {
    now: Option<u32>,
    levels: std::collections::HashMap<u32, f32>,
    rival_since: Option<Instant>,
}

impl Pick {
    fn hear(&mut self, id: u32, s: &[f32]) -> bool {
        let e = s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32;
        let k = (s.len() as f32 / 48000.0 / 0.5).min(1.0);
        let l = self.levels.entry(id).or_insert(0.0);
        *l += (e - *l) * k;
        let level = *l;
        let cur = self.now.and_then(|n| self.levels.get(&n)).copied().unwrap_or(0.0);
        if Some(id) != self.now && level > 1e-7 {
            if cur < 1e-7 {
                self.now = Some(id);
                self.rival_since = None;
            } else if level > cur * 10.0 {
                let since = *self.rival_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(2) {
                    self.now = Some(id);
                    self.rival_since = None;
                }
            } else {
                self.rival_since = None;
            }
        }
        Some(id) == self.now
    }
    fn forget(&mut self, id: u32) {
        self.levels.remove(&id);
        if self.now == Some(id) {
            self.now = None;
        }
    }
}

fn say(out: &mut impl Write, e: &Event, stamp: bool) -> std::io::Result<()> {
    if stamp {
        write!(out, "{:.3} ", e.at)?;
    }
    if e.kind == Kind::Beat {
        writeln!(out, "B")
    } else {
        writeln!(out, "{} {:.2}", letter(e.kind), e.strength)
    }
}

fn file(path: &str) -> std::io::Result<()> {
    let bytes = std::fs::read(path)?;
    let s: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let (mut ear, mut ev) = (Ear::default(), vec![]);
    let mut out = std::io::stdout().lock();
    ear.feed(&s, &mut ev);
    for e in &ev {
        say(&mut out, e, true)?;
    }
    Ok(())
}

fn live() -> std::io::Result<()> {
    die_with_parent();
    // no sound server to ask: say so by leaving, and the shell hears through cava instead
    if !Command::new("pactl").arg("info").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()) {
        eprintln!("angel-ear: pactl can't reach a sound server");
        std::process::exit(1);
    }
    let (tx, rx) = mpsc::channel();
    let mut kids: std::collections::HashMap<u32, Child> = Default::default();
    let (mut ear, mut pick, mut ev) = (Ear::default(), Pick::default(), vec![]);
    let mut out = std::io::stdout().lock();
    let (mut scanned, mut framed) = (Instant::now() - Duration::from_secs(9), Instant::now());
    loop {
        if scanned.elapsed() >= Duration::from_secs(1) {
            scanned = Instant::now();
            let found = scan();
            kids.retain(|id, kid| {
                let keep = found.contains(id) && matches!(kid.try_wait(), Ok(None));
                if !keep {
                    let _ = kid.kill();
                    let _ = kid.wait();
                    pick.forget(*id);
                }
                keep
            });
            for id in found {
                if !kids.contains_key(&id) {
                    if let Some(kid) = listen(id, tx.clone()) {
                        kids.insert(id, kid);
                    }
                }
            }
        }
        if let Ok((id, s)) = rx.recv_timeout(Duration::from_millis(100)) {
            if pick.hear(id, &s) {
                ear.feed(&s, &mut ev);
                for e in ev.drain(..) {
                    say(&mut out, &e, false)?;
                    out.flush()?;
                }
            }
        }
        if framed.elapsed() >= Duration::from_millis(33) {
            framed = Instant::now();
            let f = ear.frame();
            writeln!(out, "F {:.2} {:.2}", f.bass, f.loud)?;
            out.flush()?;
        }
    }
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--version") => {
            println!("angel-ear {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--file") => file(args.get(2).map(String::as_str).unwrap_or("/dev/stdin")),
        _ => live(),
    }
}

#[cfg(test)]
mod tests {
    use super::streams_in;
    #[test]
    fn only_programs_that_play_music() {
        let text = "Sink Input #1\n\tCorked: no\n\tProperties:\n\t\tapplication.name = \"Spotify\"\n\
            Sink Input #2\n\tCorked: yes\n\t\tapplication.name = \"Helium\"\n\
            Sink Input #3\n\tCorked: no\n\t\tnode.name = \"rcp\"\n\t\tnode.virtual = \"true\"\n\
            Sink Input #4\n\tCorked: no\n\t\tnode.name = \"quickshell\"\n\
            Sink Input #5\n\tCorked: no\n\t\tapplication.name = \"Discord\"\n\
            Sink Input #6\n\tCorked: no\n\t\tapplication.name = \"angelos-sfx\"\n";
        assert_eq!(streams_in(text), vec![1]);
    }
}
