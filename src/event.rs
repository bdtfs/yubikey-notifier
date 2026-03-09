use std::fmt;
use std::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direction {
    ToScdaemon,
    FromScdaemon,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Direction::ToScdaemon => write!(f, "->"),
            Direction::FromScdaemon => write!(f, "<-"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    ProxyStarted,
    ProxyFinished { exit_code: i32 },
    TouchRequired { command: String },
    TouchCompleted { success: bool },
    AlertStarted,
    AlertStopped,
    LineForwarded { direction: Direction, line: String },
}

/// A sink for events. Clone-friendly, send from any thread.
#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::Sender<Event>,
}

impl EventSink {
    pub fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}

/// Create an event channel. Returns a sink (for producers) and receiver (for consumers).
pub fn channel() -> (EventSink, mpsc::Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    (EventSink { tx }, rx)
}

/// Collect all events from a receiver into a Vec (useful for tests).
pub fn collect_events(rx: mpsc::Receiver<Event>) -> Vec<Event> {
    rx.try_iter().collect()
}
