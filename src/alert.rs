use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::event::{Event, EventSink};
use crate::sound::{BURST_COUNT, BURST_DELAY_MS, BURST_PAUSE_MS, ERROR_SOUND, SUCCESS_SOUND};

/// Handle that stops an alert when dropped.
pub struct AlertHandle {
    stop: Arc<AtomicBool>,
}

impl AlertHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for AlertHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Trait for alert implementations. Must be Send + Sync for sharing across threads.
pub trait Alerter: Send + Sync {
    fn start(&self) -> AlertHandle;
    /// Play a one-shot sound on operation completion (success or error).
    fn play_completion(&self, success: bool);
}

/// macOS alerter using afplay and osascript.
pub struct MacAlerter {
    pub sound: String,
    pub volume: String,
    pub events: EventSink,
}

impl MacAlerter {
    fn play_sound_blocking(sound: &str, volume: &str) {
        let _ = Command::new("afplay")
            .args(["-v", volume, sound])
            .status();
    }
}

impl Alerter for MacAlerter {
    fn play_completion(&self, success: bool) {
        let sound = if success { SUCCESS_SOUND } else { ERROR_SOUND };
        // Play synchronously - ensures the sound completes before process exit
        Self::play_sound_blocking(sound, &self.volume);
    }

    fn start(&self) -> AlertHandle {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let sound = self.sound.clone();
        let volume = self.volume.clone();
        let events = self.events.clone();

        // Show macOS notification
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(r#"display notification "Touch your YubiKey!" with title "YubiKey" sound name "Purr""#)
            .spawn();

        events.emit(Event::AlertStarted);

        thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                for i in 0..BURST_COUNT {
                    if stop_clone.load(Ordering::Relaxed) {
                        break;
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
                            events.emit(Event::AlertStopped);
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
                        sleep_interruptible(&stop_clone, BURST_DELAY_MS);
                    }
                }
                sleep_interruptible(&stop_clone, BURST_PAUSE_MS);
            }
            events.emit(Event::AlertStopped);
        });

        AlertHandle { stop }
    }
}

/// Sleep in small increments, checking the stop flag.
fn sleep_interruptible(stop: &AtomicBool, total_ms: u64) {
    let steps = total_ms / 20;
    for _ in 0..steps {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// A no-op alerter for testing. Records start/stop via events.
#[cfg(test)]
pub struct RecordingAlerter {
    pub events: EventSink,
}

#[cfg(test)]
impl Alerter for RecordingAlerter {
    fn play_completion(&self, _success: bool) {
        // no-op in tests
    }

    fn start(&self) -> AlertHandle {
        self.events.emit(Event::AlertStarted);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(10));
            }
            events.emit(Event::AlertStopped);
        });
        AlertHandle { stop }
    }
}
