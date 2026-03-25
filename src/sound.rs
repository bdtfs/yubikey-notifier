pub const DEFAULT_SOUND: &str = "/System/Library/Sounds/Funk.aiff";
pub const DEFAULT_VOLUME: &str = "2.0";
pub const BURST_COUNT: usize = 3;
pub const BURST_DELAY_MS: u64 = 400;
pub const BURST_PAUSE_MS: u64 = 2000;
pub const GRACE_PERIOD_MS: u64 = 1000;
pub const ERROR_SOUND: &str = "/System/Library/Sounds/Basso.aiff";
pub const SUCCESS_SOUND: &str = "/System/Library/Sounds/Glass.aiff";

pub const SOUNDS: &[&str] = &[
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

pub fn resolve_sound(name: &str) -> String {
    if name.starts_with('/') {
        return name.to_string();
    }
    let base = name.trim_end_matches(".aiff");
    format!("/System/Library/Sounds/{base}.aiff")
}

pub fn list_sounds() {
    eprintln!("Available macOS system sounds:");
    for s in SOUNDS {
        let name = s.rsplit('/').next().unwrap_or(s);
        eprintln!("  {name}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_sound_by_name() {
        assert_eq!(resolve_sound("Funk"), "/System/Library/Sounds/Funk.aiff");
    }

    #[test]
    fn resolve_sound_with_extension() {
        assert_eq!(
            resolve_sound("Funk.aiff"),
            "/System/Library/Sounds/Funk.aiff"
        );
    }

    #[test]
    fn resolve_sound_absolute_path() {
        assert_eq!(resolve_sound("/custom/path.aiff"), "/custom/path.aiff");
    }
}
