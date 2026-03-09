use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub scdaemon: Option<String>,
    pub sound: String,
    pub volume: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            scdaemon: None,
            sound: crate::sound::DEFAULT_SOUND.to_string(),
            volume: crate::sound::DEFAULT_VOLUME.to_string(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        Self::load_from(&default_config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        let map = match fs::read_to_string(path) {
            Ok(contents) => parse_config(&contents),
            Err(_) => HashMap::new(),
        };
        Config {
            scdaemon: map.get("scdaemon").filter(|s| !s.is_empty()).cloned(),
            sound: map
                .get("sound")
                .cloned()
                .unwrap_or_else(|| crate::sound::DEFAULT_SOUND.to_string()),
            volume: map
                .get("volume")
                .cloned()
                .unwrap_or_else(|| crate::sound::DEFAULT_VOLUME.to_string()),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let dir = default_config_dir();
        fs::create_dir_all(&dir)?;
        self.save_to(&default_config_path())
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = self.serialize();
        fs::write(path, contents)
    }

    pub fn serialize(&self) -> String {
        let mut lines = Vec::new();
        if let Some(ref scd) = self.scdaemon {
            lines.push(format!("scdaemon={scd}"));
        }
        lines.push(format!("sound={}", self.sound));
        lines.push(format!("volume={}", self.volume));
        lines.push(String::new()); // trailing newline
        lines.join("\n")
    }
}

pub fn parse_config(contents: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

pub fn default_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".config/yubikey-notifier")
}

pub fn default_config_path() -> PathBuf {
    default_config_dir().join("config")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty() {
        let map = parse_config("");
        assert!(map.is_empty());
    }

    #[test]
    fn parse_comments_and_whitespace() {
        let input = "# comment\n  \n  key = value  \n";
        let map = parse_config(input);
        assert_eq!(map.get("key").unwrap(), "value");
    }

    #[test]
    fn parse_multiple_keys() {
        let input = "scdaemon=/usr/bin/scdaemon\nsound=Funk.aiff\nvolume=1.5\n";
        let map = parse_config(input);
        assert_eq!(map.get("scdaemon").unwrap(), "/usr/bin/scdaemon");
        assert_eq!(map.get("sound").unwrap(), "Funk.aiff");
        assert_eq!(map.get("volume").unwrap(), "1.5");
    }

    #[test]
    fn config_round_trip() {
        let config = Config {
            scdaemon: Some("/usr/bin/scdaemon".to_string()),
            sound: "/System/Library/Sounds/Funk.aiff".to_string(),
            volume: "1.5".to_string(),
        };
        let serialized = config.serialize();
        let map = parse_config(&serialized);
        assert_eq!(map.get("scdaemon").unwrap(), "/usr/bin/scdaemon");
        assert_eq!(map.get("sound").unwrap(), "/System/Library/Sounds/Funk.aiff");
        assert_eq!(map.get("volume").unwrap(), "1.5");
    }

    #[test]
    fn config_round_trip_no_scdaemon() {
        let config = Config {
            scdaemon: None,
            ..Config::default()
        };
        let serialized = config.serialize();
        let map = parse_config(&serialized);
        assert!(!map.contains_key("scdaemon"));
    }

    #[test]
    fn load_from_missing_file() {
        let config = Config::load_from(Path::new("/nonexistent/path/config"));
        assert_eq!(config, Config::default());
    }
}
