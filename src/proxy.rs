use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::alert::{AlertHandle, Alerter};
use crate::event::{Event, EventSink};
use crate::sound::GRACE_PERIOD_MS;

/// Returns true if the line is a command that requires YubiKey touch.
pub fn is_touch_command(line: &str) -> bool {
    let upper = line.trim().to_uppercase();
    upper.starts_with("PKSIGN")
        || upper.starts_with("PKDECRYPT")
        || upper.starts_with("PKAUTH")
}

/// Returns true if the line is a completion response from scdaemon.
pub fn is_completion(line: &str) -> Option<bool> {
    let trimmed = line.trim();
    if trimmed.starts_with("OK") {
        Some(true)
    } else if trimmed.starts_with("ERR") {
        Some(false)
    } else {
        None
    }
}

/// State machine for tracking touch detection with grace period.
enum TouchState {
    /// No touch operation in progress.
    Idle,
    /// Touch command seen, waiting for grace period before alerting.
    Pending { since: Instant, command: String },
    /// Grace period expired, alert is active.
    Alerting { _handle: AlertHandle },
}

struct ProxyState {
    touch: TouchState,
}

impl ProxyState {
    fn new() -> Self {
        Self {
            touch: TouchState::Idle,
        }
    }

    fn is_pending_or_alerting(&self) -> bool {
        !matches!(self.touch, TouchState::Idle)
    }
}

/// Run the bidirectional proxy between gpg-agent and scdaemon.
///
/// - `agent_read`: lines coming from gpg-agent (stdin in production)
/// - `agent_write`: lines going to gpg-agent (stdout in production)
/// - `scd_read`: lines coming from scdaemon (child stdout)
/// - `scd_write`: lines going to scdaemon (child stdin)
///
/// Returns when either side closes.
pub fn run_proxy<AR, AW, SR, SW>(
    agent_read: AR,
    agent_write: AW,
    scd_read: SR,
    scd_write: SW,
    alerter: Arc<dyn Alerter>,
    events: EventSink,
) where
    AR: BufRead + Send + 'static,
    AW: Write + Send + 'static,
    SR: BufRead + Send + 'static,
    SW: Write + Send + 'static,
{
    let state = Arc::new((Mutex::new(ProxyState::new()), Condvar::new()));
    let shutdown = Arc::new(AtomicBool::new(false));

    events.emit(Event::ProxyStarted);

    // Grace period watcher thread
    let grace_state = state.clone();
    let grace_alerter = alerter.clone();
    let grace_events = events.clone();
    let grace_shutdown = shutdown.clone();
    let _grace_thread = thread::spawn(move || {
        let (lock, cvar) = &*grace_state;
        loop {
            if grace_shutdown.load(Ordering::Relaxed) {
                return;
            }
            let deadline = {
                let st = lock.lock().unwrap();
                match &st.touch {
                    TouchState::Pending { since, .. } => {
                        *since + Duration::from_millis(GRACE_PERIOD_MS)
                    }
                    _ => {
                        // Wait until notified (state change to Pending)
                        let st = cvar.wait(st).unwrap();
                        match &st.touch {
                            TouchState::Pending { since, .. } => {
                                *since + Duration::from_millis(GRACE_PERIOD_MS)
                            }
                            _ => continue, // Spurious wake or already completed
                        }
                    }
                }
            };

            // Sleep until grace period expires, waking on state changes
            {
                let mut st = lock.lock().unwrap();
                loop {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    if !matches!(st.touch, TouchState::Pending { .. }) {
                        break; // State changed (completed during grace period)
                    }
                    let remaining = deadline - now;
                    let result = cvar.wait_timeout(st, remaining).unwrap();
                    st = result.0;
                }

                // If still pending after grace period, escalate to alerting
                let cmd = match &st.touch {
                    TouchState::Pending { command, .. } => Some(command.clone()),
                    _ => None,
                };
                if let Some(command) = cmd {
                    grace_events.emit(Event::TouchRequired { command });
                    let handle = grace_alerter.start();
                    st.touch = TouchState::Alerting { _handle: handle };
                }
            }
        }
    });

    // Forward: agent -> scdaemon
    let fwd_state = state.clone();
    let _events_fwd = events.clone();
    let _fwd_thread = thread::spawn(move || {
        let (lock, cvar) = &*fwd_state;
        let mut writer = scd_write;
        let mut agent_read = agent_read;
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match agent_read.read_until(b'\n', &mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {}
                Err(_) => break,
            }

            // Try to interpret as UTF-8 for touch detection
            let line_str = String::from_utf8_lossy(&buf);
            let line_trimmed = line_str.trim_end_matches('\n').trim_end_matches('\r');

            #[cfg(debug_assertions)]
            _events_fwd.emit(Event::LineForwarded {
                direction: crate::event::Direction::ToScdaemon,
                line: line_trimmed.to_string(),
            });

            if is_touch_command(line_trimmed) {
                let mut st = lock.lock().unwrap();
                if !st.is_pending_or_alerting() {
                    st.touch = TouchState::Pending {
                        since: Instant::now(),
                        command: line_trimmed.trim().to_string(),
                    };
                    cvar.notify_all();
                }
            }

            // Forward raw bytes (preserving original encoding)
            if writer.write_all(&buf).is_err() {
                break;
            }
            if writer.flush().is_err() {
                break;
            }
        }
    });

    // Forward: scdaemon -> agent
    let back_state = state.clone();
    let back_shutdown = shutdown.clone();
    let back_alerter = alerter.clone();
    let events_back = events.clone();
    let back_thread = thread::spawn(move || {
        let (lock, cvar) = &*back_state;
        let mut writer = agent_write;
        let mut scd_read = scd_read;
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match scd_read.read_until(b'\n', &mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {}
                Err(_) => break,
            }

            let line_str = String::from_utf8_lossy(&buf);
            let line_trimmed = line_str.trim_end_matches('\n').trim_end_matches('\r');

            #[cfg(debug_assertions)]
            events_back.emit(Event::LineForwarded {
                direction: crate::event::Direction::FromScdaemon,
                line: line_trimmed.to_string(),
            });

            // Check for completion while in pending or alerting state
            {
                let mut st = lock.lock().unwrap();
                if st.is_pending_or_alerting() {
                    if let Some(success) = is_completion(line_trimmed) {
                        let was_alerting = matches!(st.touch, TouchState::Alerting { .. });
                        // Reset to idle - drops AlertHandle if alerting (stops sound)
                        st.touch = TouchState::Idle;
                        cvar.notify_all();
                        events_back.emit(Event::TouchCompleted { success });
                        // Play completion sound only if the alert was visible to the user
                        if was_alerting {
                            back_alerter.play_completion(success);
                        }
                    }
                }
            }

            // Forward raw bytes (preserving original encoding)
            if writer.write_all(&buf).is_err() {
                break;
            }
            if writer.flush().is_err() {
                break;
            }
        }
        // scdaemon disconnected - clean up and signal shutdown
        back_shutdown.store(true, Ordering::Relaxed);
        let (lock, cvar) = &*back_state;
        let mut st = lock.lock().unwrap();
        let was_alerting = matches!(st.touch, TouchState::Alerting { .. });
        st.touch = TouchState::Idle;
        cvar.notify_all();
        // If scdaemon died/disconnected while alert was playing, treat as error
        if was_alerting {
            events_back.emit(Event::TouchCompleted { success: false });
            back_alerter.play_completion(false);
        }
    });

    // Wait for either side to close. If scdaemon exits, we're done -
    // don't block waiting for the agent side (fwd_thread may be stuck
    // reading from gpg-agent which keeps the pipe open).
    let _ = back_thread.join();
    // Signal everything to stop
    shutdown.store(true, Ordering::Relaxed);
    let (_, cvar) = &*state;
    cvar.notify_all();
    // fwd_thread and grace_thread will exit when the process exits
    // or when their next I/O operation fails.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alert::RecordingAlerter;
    use crate::event;
    use std::io::Cursor;

    fn make_test_alerter(events: &EventSink) -> Arc<dyn Alerter> {
        Arc::new(RecordingAlerter {
            events: events.clone(),
        })
    }

    #[test]
    fn test_is_touch_command() {
        assert!(is_touch_command("PKSIGN"));
        assert!(is_touch_command("PKSIGN --hash=sha256"));
        assert!(is_touch_command("PKDECRYPT"));
        assert!(is_touch_command("PKAUTH"));
        assert!(is_touch_command("  pksign  ")); // case insensitive, trimmed
        assert!(!is_touch_command("READKEY"));
        assert!(!is_touch_command("LEARN"));
        assert!(!is_touch_command("OK"));
    }

    #[test]
    fn test_is_completion() {
        assert_eq!(is_completion("OK"), Some(true));
        assert_eq!(is_completion("OK Finished"), Some(true));
        assert_eq!(is_completion("ERR 123 error"), Some(false));
        assert_eq!(is_completion("D some data"), None);
        assert_eq!(is_completion("S PROGRESS"), None);
    }

    #[test]
    fn proxy_forwards_lines() {
        let (sink, rx) = event::channel();
        let alerter = make_test_alerter(&sink);

        let agent_input = Cursor::new(b"LEARN\nREADKEY\n".to_vec());
        let agent_output: Vec<u8> = Vec::new();
        let scd_input = Cursor::new(b"OK\nOK\n".to_vec());
        let scd_output: Vec<u8> = Vec::new();

        let agent_output = Arc::new(Mutex::new(agent_output));
        let scd_output = Arc::new(Mutex::new(scd_output));

        let ao = SharedWriter(agent_output.clone());
        let so = SharedWriter(scd_output.clone());

        run_proxy(agent_input, ao, scd_input, so, alerter, sink);

        let forwarded_to_scd = String::from_utf8(scd_output.lock().unwrap().clone()).unwrap();
        assert!(forwarded_to_scd.contains("LEARN"));
        assert!(forwarded_to_scd.contains("READKEY"));

        let forwarded_to_agent = String::from_utf8(agent_output.lock().unwrap().clone()).unwrap();
        assert!(forwarded_to_agent.contains("OK"));

        let events = event::collect_events(rx);
        assert!(events.contains(&Event::ProxyStarted));
        // No touch commands, so no TouchRequired
        assert!(!events.iter().any(|e| matches!(e, Event::TouchRequired { .. })));
    }

    #[test]
    fn proxy_no_alert_on_fast_completion() {
        // If scdaemon responds OK before grace period, no alert should fire.
        // Use pipes to control timing: send PKSIGN, wait for it to be processed,
        // then send OK.
        use std::io::Write as _;

        let (sink, rx) = event::channel();
        let alerter = make_test_alerter(&sink);

        let agent_input = Cursor::new(b"PKSIGN --hash=sha256\n".to_vec());

        let (scd_read_end, mut scd_write_end) = os_pipe::pipe().unwrap();
        let (_agent_read_end, agent_write_end) = os_pipe::pipe().unwrap();

        let sink_clone = sink.clone();
        let handle = thread::spawn(move || {
            run_proxy(
                agent_input,
                agent_write_end,
                std::io::BufReader::new(scd_read_end),
                SharedWriter(Arc::new(Mutex::new(Vec::new()))),
                alerter,
                sink_clone,
            );
        });

        // Give fwd_thread time to process PKSIGN and set state to Pending
        thread::sleep(Duration::from_millis(100));

        // Send fast OK (within grace period)
        writeln!(scd_write_end, "OK").unwrap();
        drop(scd_write_end);

        handle.join().unwrap();
        thread::sleep(Duration::from_millis(50));

        let events = event::collect_events(rx);
        assert!(events.contains(&Event::ProxyStarted));
        assert!(events.contains(&Event::TouchCompleted { success: true }));
        // No alert should have fired - completion came before grace period
        assert!(!events.iter().any(|e| matches!(e, Event::AlertStarted)));
    }

    #[test]
    fn proxy_no_alert_on_fast_error() {
        use std::io::Write as _;

        let (sink, rx) = event::channel();
        let alerter = make_test_alerter(&sink);

        let agent_input = Cursor::new(b"PKDECRYPT\n".to_vec());

        let (scd_read_end, mut scd_write_end) = os_pipe::pipe().unwrap();
        let (_agent_read_end, agent_write_end) = os_pipe::pipe().unwrap();

        let sink_clone = sink.clone();
        let handle = thread::spawn(move || {
            run_proxy(
                agent_input,
                agent_write_end,
                std::io::BufReader::new(scd_read_end),
                SharedWriter(Arc::new(Mutex::new(Vec::new()))),
                alerter,
                sink_clone,
            );
        });

        thread::sleep(Duration::from_millis(100));

        writeln!(scd_write_end, "ERR 100 failed").unwrap();
        drop(scd_write_end);

        handle.join().unwrap();
        thread::sleep(Duration::from_millis(50));

        let events = event::collect_events(rx);
        assert!(events.contains(&Event::TouchCompleted { success: false }));
        assert!(!events.iter().any(|e| matches!(e, Event::AlertStarted)));
    }

    #[test]
    fn proxy_alerts_after_grace_period() {
        // Use a pipe to control timing - scdaemon doesn't respond until after grace period
        use std::io::Write as _;

        let (sink, rx) = event::channel();
        let alerter = make_test_alerter(&sink);

        let agent_input = Cursor::new(b"PKSIGN --hash=sha256\n".to_vec());

        // Use a real pipe for scd_read so we can delay the response
        let (scd_read_end, mut scd_write_end) = os_pipe::pipe().unwrap();
        let (_agent_read_pipe, agent_write_pipe) = os_pipe::pipe().unwrap();

        let sink_clone = sink.clone();
        let handle = thread::spawn(move || {
            run_proxy(
                agent_input,
                agent_write_pipe,
                std::io::BufReader::new(scd_read_end),
                SharedWriter(Arc::new(Mutex::new(Vec::new()))),
                alerter,
                sink_clone,
            );
        });

        // Wait longer than grace period
        thread::sleep(Duration::from_millis(GRACE_PERIOD_MS + 200));

        // Now send completion
        writeln!(scd_write_end, "OK").unwrap();
        drop(scd_write_end); // Close pipe to let proxy exit

        handle.join().unwrap();

        thread::sleep(Duration::from_millis(50));

        let events = event::collect_events(rx);
        assert!(events.contains(&Event::ProxyStarted));
        assert!(events.contains(&Event::TouchRequired {
            command: "PKSIGN --hash=sha256".to_string(),
        }));
        assert!(events.contains(&Event::AlertStarted));
        assert!(events.contains(&Event::TouchCompleted { success: true }));
        assert!(events.contains(&Event::AlertStopped));
    }

    /// A Write impl backed by a shared Vec<u8> for testing.
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
