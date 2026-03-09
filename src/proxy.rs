use std::io::{BufRead, Write};
use std::sync::Arc;
use std::thread;

use crate::alert::{AlertHandle, Alerter};
use crate::event::{Event, EventSink};

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
    use std::sync::Mutex;

    let alert_handle: Arc<Mutex<Option<AlertHandle>>> = Arc::new(Mutex::new(None));

    events.emit(Event::ProxyStarted);

    // Forward: agent -> scdaemon
    let alert_fwd = alert_handle.clone();
    let alerter_fwd = alerter.clone();
    let events_fwd = events.clone();
    let fwd_thread = thread::spawn(move || {
        let mut writer = scd_write;
        for line in agent_read.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            #[cfg(debug_assertions)]
            events_fwd.emit(Event::LineForwarded {
                direction: crate::event::Direction::ToScdaemon,
                line: line.clone(),
            });

            if is_touch_command(&line) {
                let mut handle = alert_fwd.lock().unwrap();
                if handle.is_none() {
                    events_fwd.emit(Event::TouchRequired {
                        command: line.trim().to_string(),
                    });
                    *handle = Some(alerter_fwd.start());
                }
            }

            if writeln!(writer, "{}", line).is_err() {
                break;
            }
            if writer.flush().is_err() {
                break;
            }
        }
    });

    // Forward: scdaemon -> agent
    let alert_back = alert_handle.clone();
    let events_back = events.clone();
    let back_thread = thread::spawn(move || {
        let mut writer = agent_write;
        for line in scd_read.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            #[cfg(debug_assertions)]
            events_back.emit(Event::LineForwarded {
                direction: crate::event::Direction::FromScdaemon,
                line: line.clone(),
            });

            // Check for completion while alert is active
            {
                let mut handle = alert_back.lock().unwrap();
                if handle.is_some() {
                    if let Some(success) = is_completion(&line) {
                        // Drop the handle to stop the alert
                        *handle = None;
                        events_back.emit(Event::TouchCompleted { success });
                    }
                }
            }

            if writeln!(writer, "{}", line).is_err() {
                break;
            }
            if writer.flush().is_err() {
                break;
            }
        }
        // Ensure alert stops if scdaemon disconnects
        let mut handle = alert_back.lock().unwrap();
        *handle = None;
    });

    let _ = fwd_thread.join();
    let _ = back_thread.join();
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

        let agent_output = std::sync::Arc::new(std::sync::Mutex::new(agent_output));
        let scd_output = std::sync::Arc::new(std::sync::Mutex::new(scd_output));

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
    fn proxy_detects_touch_and_completion() {
        let (sink, rx) = event::channel();
        let alerter = make_test_alerter(&sink);

        // Simulate: agent sends PKSIGN, scdaemon responds with OK
        let agent_input = Cursor::new(b"PKSIGN --hash=sha256\n".to_vec());
        let scd_input = Cursor::new(b"OK\n".to_vec());

        let agent_output = Arc::new(std::sync::Mutex::new(Vec::new()));
        let scd_output = Arc::new(std::sync::Mutex::new(Vec::new()));

        run_proxy(
            agent_input,
            SharedWriter(agent_output.clone()),
            scd_input,
            SharedWriter(scd_output.clone()),
            alerter,
            sink,
        );

        // Give threads a moment to emit events
        std::thread::sleep(std::time::Duration::from_millis(50));

        let events = event::collect_events(rx);
        assert!(events.contains(&Event::ProxyStarted));
        assert!(events.contains(&Event::TouchRequired {
            command: "PKSIGN --hash=sha256".to_string(),
        }));
        assert!(events.contains(&Event::AlertStarted));
        assert!(events.contains(&Event::TouchCompleted { success: true }));
        assert!(events.contains(&Event::AlertStopped));
    }

    #[test]
    fn proxy_detects_error_completion() {
        let (sink, rx) = event::channel();
        let alerter = make_test_alerter(&sink);

        let agent_input = Cursor::new(b"PKDECRYPT\n".to_vec());
        let scd_input = Cursor::new(b"ERR 100 failed\n".to_vec());

        run_proxy(
            agent_input,
            SharedWriter(Arc::new(std::sync::Mutex::new(Vec::new()))),
            scd_input,
            SharedWriter(Arc::new(std::sync::Mutex::new(Vec::new()))),
            alerter,
            sink,
        );

        std::thread::sleep(std::time::Duration::from_millis(50));

        let events = event::collect_events(rx);
        assert!(events.contains(&Event::TouchCompleted { success: false }));
    }

    /// A Write impl backed by a shared Vec<u8> for testing.
    struct SharedWriter(Arc<std::sync::Mutex<Vec<u8>>>);

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
