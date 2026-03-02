use pcsc::*;
use std::collections::HashSet;
use std::ffi::CString;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

// --- Process monitoring ---

fn get_signing_pids() -> HashSet<u32> {
    let mut pids = HashSet::new();
    for name in &["gpg", "gpg2", "ssh"] {
        if let Ok(output) = Command::new("pgrep").arg("-x").arg(name).output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if let Ok(pid) = line.trim().parse::<u32>() {
                    pids.insert(pid);
                }
            }
        }
    }
    pids
}

// --- PCSC ---

fn find_yubikey_reader(ctx: &Context) -> Option<CString> {
    let readers = ctx.list_readers_owned().ok()?;
    for reader in &readers {
        let name = reader.to_str().unwrap_or("");
        if name.to_lowercase().contains("yubi") {
            return Some(reader.clone());
        }
    }
    None
}

fn probe_loop(heartbeat_tx: mpsc::Sender<ProbeStatus>) {
    loop {
        let _ = heartbeat_tx.send(probe_once());
        thread::sleep(Duration::from_millis(50));
    }
}

fn probe_once() -> ProbeStatus {
    let ctx = match Context::establish(Scope::System) {
        Ok(c) => c,
        Err(_) => return ProbeStatus::NoReader,
    };

    let reader = match find_yubikey_reader(&ctx) {
        Some(r) => r,
        None => return ProbeStatus::NoReader,
    };

    let mut card = match ctx.connect(&reader, ShareMode::Shared, Protocols::ANY) {
        Ok(c) => c,
        Err(Error::SharingViolation) => return ProbeStatus::Busy,
        Err(_) => return ProbeStatus::NoReader,
    };

    let result = card.transaction();
    let status = match &result {
        Ok(_) => ProbeStatus::Idle,
        Err(_) => ProbeStatus::Busy,
    };
    drop(result);
    status
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ProbeStatus {
    Idle,
    Busy,
    NoReader,
}

// --- Sound ---

const DEFAULT_SOUND: &str = "/System/Library/Sounds/Funk.aiff";
const FAILURE_SOUND: &str = "/System/Library/Sounds/Basso.aiff";
const DEFAULT_VOLUME: &str = "2.0";
const BURST_COUNT: usize = 3;
const BURST_DELAY_MS: u64 = 400;
const BURST_PAUSE_MS: u64 = 2000;

const SOUNDS: &[&str] = &[
    "/System/Library/Sounds/Funk.aiff",
    "/System/Library/Sounds/Basso.aiff",
    "/System/Library/Sounds/Blow.aiff",
    "/System/Library/Sounds/Bottle.aiff",
    "/System/Library/Sounds/Frog.aiff",
    "/System/Library/Sounds/Glass.aiff",
    "/System/Library/Sounds/Hero.aiff",
    "/System/Library/Sounds/Morse.aiff",
    "/System/Library/Sounds/Ping.aiff",
    "/System/Library/Sounds/Pop.aiff",
    "/System/Library/Sounds/Purr.aiff",
    "/System/Library/Sounds/Sosumi.aiff",
    "/System/Library/Sounds/Submarine.aiff",
    "/System/Library/Sounds/Tink.aiff",
];

fn start_alert_loop(sound: &str, volume: &str) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();
    let sound = sound.to_string();
    let volume = volume.to_string();

    let _ = Command::new("osascript")
        .arg("-e")
        .arg(r#"display notification "Touch your YubiKey!" with title "YubiKey" sound name "Purr""#)
        .spawn();

    thread::spawn(move || {
        while !stop_clone.load(Ordering::Relaxed) {
            for i in 0..BURST_COUNT {
                if stop_clone.load(Ordering::Relaxed) {
                    return;
                }
                let mut child = match Command::new("afplay")
                    .args(["-v", &volume, &sound])
                    .spawn()
                {
                    Ok(c) => c,
                    Err(_) => return,
                };
                loop {
                    if stop_clone.load(Ordering::Relaxed) {
                        let _ = child.kill();
                        return;
                    }
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => {}
                        Err(_) => break,
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                if i < BURST_COUNT - 1 {
                    for _ in 0..(BURST_DELAY_MS / 20) {
                        if stop_clone.load(Ordering::Relaxed) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                }
            }
            for _ in 0..(BURST_PAUSE_MS / 20) {
                if stop_clone.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    });

    stop
}

fn play_failure_sound(volume: &str) {
    let volume = volume.to_string();
    thread::spawn(move || {
        for _ in 0..2 {
            let _ = Command::new("afplay")
                .args(["-v", &volume, FAILURE_SOUND])
                .status();
        }
    });
}

fn stop_alert(alert: &mut Option<Arc<AtomicBool>>) {
    if let Some(flag) = alert.take() {
        flag.store(true, Ordering::Relaxed);
    }
}

// --- CLI ---

fn print_usage() {
    eprintln!("Usage: yubikey-notifier [OPTIONS]\n");
    eprintln!("Options:");
    eprintln!("  --sound <NAME>     Alert sound (default: Funk). Failure always plays Basso.");
    eprintln!("  --volume <FLOAT>   Volume multiplier, 1.0 = normal (default: 2.0)");
    eprintln!("  --list-sounds      List available macOS system sounds");
    eprintln!("  --help             Show this help");
}

fn list_sounds() {
    eprintln!("Available macOS system sounds:");
    for s in SOUNDS {
        let name = s.rsplit('/').next().unwrap_or(s);
        eprintln!("  {name}");
    }
}

fn resolve_sound(name: &str) -> String {
    if name.starts_with('/') {
        return name.to_string();
    }
    let base = name.trim_end_matches(".aiff");
    format!("/System/Library/Sounds/{base}.aiff")
}

// --- Main ---

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut sound = DEFAULT_SOUND.to_string();
    let mut volume = DEFAULT_VOLUME.to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                return;
            }
            "--list-sounds" => {
                list_sounds();
                return;
            }
            "--sound" => {
                i += 1;
                sound = resolve_sound(args.get(i).map(|s| s.as_str()).unwrap_or("Funk"));
            }
            "--volume" => {
                i += 1;
                volume = args.get(i).cloned().unwrap_or_else(|| DEFAULT_VOLUME.to_string());
            }
            other => {
                eprintln!("Unknown option: {other}");
                print_usage();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    eprintln!("yubikey-notifier: watching for touch prompts...");
    eprintln!("  Sound: {} (volume: {}x)", sound, volume);
    eprintln!("  Press Ctrl+C to stop\n");

    let (heartbeat_tx, heartbeat_rx) = mpsc::channel();
    thread::spawn(move || probe_loop(heartbeat_tx));

    let mut alert_stop: Option<Arc<AtomicBool>> = None;
    let mut last_heartbeat = Instant::now();
    let mut reader_found = false;
    let mut busy_since: Option<Instant> = None;
    let mut seen_signing_pids: HashSet<u32> = HashSet::new();
    let mut cooldown_until: Option<Instant> = None;

    loop {
        while let Ok(status) = heartbeat_rx.try_recv() {
            last_heartbeat = Instant::now();

            match status {
                ProbeStatus::Idle => {
                    if !reader_found {
                        eprintln!("  YubiKey detected");
                        reader_found = true;
                    }
                    if cooldown_until.is_some() {
                        cooldown_until = None;
                    }
                    busy_since = None;
                    if alert_stop.is_some() {
                        stop_alert(&mut alert_stop);
                        seen_signing_pids.clear();
                    }
                }
                ProbeStatus::Busy => {
                    let in_cooldown = cooldown_until
                        .map(|t| Instant::now() < t)
                        .unwrap_or(false);
                    // Only start the alert countdown if a signing process is running.
                    // gpg-agent/scdaemon grab the card periodically for housekeeping —
                    // we must not alert on that.
                    if alert_stop.is_none() && !in_cooldown && busy_since.is_none() {
                        let pids = get_signing_pids();
                        if !pids.is_empty() {
                            busy_since = Some(Instant::now());
                            seen_signing_pids = pids;
                        }
                    }
                }
                ProbeStatus::NoReader => {
                    if reader_found {
                        eprintln!("  YubiKey disconnected");
                        reader_found = false;
                    }
                    if alert_stop.is_some() {
                        stop_alert(&mut alert_stop);
                        play_failure_sound(&volume);
                        busy_since = None;
                        seen_signing_pids.clear();
                    }
                }
            }
        }

        // Start alert after 1s of sustained busy (lets YubiKey start blinking first)
        if alert_stop.is_none() {
            if let Some(since) = busy_since {
                if since.elapsed() > Duration::from_secs(1) {
                    eprintln!("  Touch your YubiKey!");
                    alert_stop = Some(start_alert_loop(&sound, &volume));
                }
            }
        }

        // Stop alert when signing process exits (instant detection)
        if alert_stop.is_some() {
            let current = get_signing_pids();
            for &pid in &current {
                seen_signing_pids.insert(pid);
            }
            if !seen_signing_pids.is_empty() && current.is_empty() {
                eprintln!("  Touch completed");
                stop_alert(&mut alert_stop);
                busy_since = None;
                seen_signing_pids.clear();
                cooldown_until = Some(Instant::now() + Duration::from_secs(300));
            }
        }

        // Blocked probe fallback — also requires a signing process
        let silence = last_heartbeat.elapsed();
        let in_cooldown = cooldown_until.map(|t| Instant::now() < t).unwrap_or(false);
        if silence > Duration::from_millis(1500) && alert_stop.is_none() && reader_found && !in_cooldown {
            let pids = get_signing_pids();
            if !pids.is_empty() {
                eprintln!("  Touch your YubiKey!");
                alert_stop = Some(start_alert_loop(&sound, &volume));
                busy_since = Some(Instant::now());
                seen_signing_pids = pids;
            }
        }

        // 30s timeout
        if let Some(since) = busy_since {
            if since.elapsed() > Duration::from_secs(30) && alert_stop.is_some() {
                eprintln!("  Timed out");
                stop_alert(&mut alert_stop);
                play_failure_sound(&volume);
                busy_since = None;
                seen_signing_pids.clear();
            }
        }

        thread::sleep(Duration::from_millis(50));
    }
}
