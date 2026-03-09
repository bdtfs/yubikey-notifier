use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;

#[derive(Debug)]
pub enum SetupError {
    ScdaemonNotFound,
    SelfReference,
    Io(std::io::Error),
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetupError::ScdaemonNotFound => {
                write!(f, "could not detect scdaemon via gpgconf --list-components")
            }
            SetupError::SelfReference => write!(
                f,
                "gpgconf reports scdaemon as our own binary -- already configured?\n\
                 Run --uninstall first, then try again."
            ),
            SetupError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl From<std::io::Error> for SetupError {
    fn from(e: std::io::Error) -> Self {
        SetupError::Io(e)
    }
}

/// The result of planning a setup operation.
#[derive(Debug)]
pub struct SetupPlan {
    pub config: Config,
    pub gpg_agent_conf_lines: Vec<String>,
}

/// Detect the real scdaemon path from gpgconf.
pub fn detect_real_scdaemon() -> Option<String> {
    let output = Command::new("gpgconf")
        .arg("--list-components")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_scdaemon_from_gpgconf(&stdout)
}

/// Parse scdaemon path from gpgconf --list-components output.
pub fn parse_scdaemon_from_gpgconf(output: &str) -> Option<String> {
    for line in output.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[0] == "scdaemon" {
            let path = parts[2].to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }
    None
}

/// Plan a setup operation (pure logic, no I/O).
pub fn plan_setup(
    current_exe: &str,
    detected_scdaemon: Option<String>,
    existing_conf: Option<&str>,
    sound: &str,
    volume: &str,
) -> Result<SetupPlan, SetupError> {
    let scdaemon_path = detected_scdaemon.ok_or(SetupError::ScdaemonNotFound)?;

    if scdaemon_path == current_exe {
        return Err(SetupError::SelfReference);
    }

    let config = Config {
        scdaemon: Some(scdaemon_path),
        sound: sound.to_string(),
        volume: volume.to_string(),
    };

    let mut lines: Vec<String> = existing_conf
        .unwrap_or("")
        .lines()
        .map(|l| l.to_string())
        .collect();

    // Remove existing scdaemon-program lines
    lines.retain(|l| {
        let trimmed = l.trim();
        !trimmed.starts_with("scdaemon-program ")
            && !trimmed.starts_with("scdaemon-program\t")
            && trimmed != "scdaemon-program"
    });

    lines.push(format!("scdaemon-program {current_exe}"));

    Ok(SetupPlan {
        config,
        gpg_agent_conf_lines: lines,
    })
}

/// Plan an uninstall: filter out scdaemon-program lines.
pub fn plan_uninstall(existing_conf: &str) -> Vec<String> {
    existing_conf
        .lines()
        .filter(|l| {
            let trimmed = l.trim();
            !trimmed.starts_with("scdaemon-program ")
                && !trimmed.starts_with("scdaemon-program\t")
                && trimmed != "scdaemon-program"
        })
        .map(|l| l.to_string())
        .collect()
}

/// Execute a setup plan: write config, update gpg-agent.conf, restart gpg-agent.
pub fn execute_setup(
    plan: &SetupPlan,
    config_path: &Path,
    gpg_agent_conf_path: &Path,
) -> Result<(), SetupError> {
    plan.config.save_to(config_path)?;
    eprintln!("Saved config to {}", config_path.display());

    if let Some(parent) = gpg_agent_conf_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        gpg_agent_conf_path,
        plan.gpg_agent_conf_lines.join("\n") + "\n",
    )?;
    eprintln!("Updated {}", gpg_agent_conf_path.display());

    restart_gpg_agent();
    eprintln!("Restarted gpg-agent");

    Ok(())
}

/// Execute uninstall: update gpg-agent.conf, restart agent, remove config.
pub fn execute_uninstall(
    gpg_agent_conf_path: &Path,
    config_path: &Path,
) -> Result<(), SetupError> {
    if gpg_agent_conf_path.exists() {
        let contents = fs::read_to_string(gpg_agent_conf_path)?;
        let lines = plan_uninstall(&contents);
        fs::write(gpg_agent_conf_path, lines.join("\n") + "\n")?;
        eprintln!(
            "Removed scdaemon-program from {}",
            gpg_agent_conf_path.display()
        );
    }

    restart_gpg_agent();
    eprintln!("Restarted gpg-agent");

    if config_path.exists() {
        let _ = fs::remove_file(config_path);
        eprintln!("Removed config {}", config_path.display());
    }
    if let Some(dir) = config_path.parent() {
        let _ = fs::remove_dir(dir);
    }

    Ok(())
}

pub fn gpg_agent_conf_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".gnupg/gpg-agent.conf")
}

fn restart_gpg_agent() {
    let _ = Command::new("gpgconf")
        .args(["--kill", "gpg-agent"])
        .status();
}

/// Get the real scdaemon path, preferring config, falling back to detection.
pub fn get_real_scdaemon(config: &Config) -> String {
    if let Some(ref path) = config.scdaemon {
        return path.clone();
    }
    detect_real_scdaemon().unwrap_or_else(|| "/usr/local/libexec/scdaemon".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gpgconf_output() {
        let output = "gpg:GPG:gpg\nscdaemon:SCDaemon:/usr/local/libexec/scdaemon\n";
        assert_eq!(
            parse_scdaemon_from_gpgconf(output),
            Some("/usr/local/libexec/scdaemon".to_string())
        );
    }

    #[test]
    fn parse_gpgconf_no_scdaemon() {
        let output = "gpg:GPG:gpg\n";
        assert_eq!(parse_scdaemon_from_gpgconf(output), None);
    }

    #[test]
    fn plan_setup_basic() {
        let plan = plan_setup(
            "/usr/local/bin/yubikey-notifier",
            Some("/usr/local/libexec/scdaemon".to_string()),
            Some("pinentry-program /usr/local/bin/pinentry\n"),
            "/System/Library/Sounds/Funk.aiff",
            "2.0",
        )
        .unwrap();

        assert_eq!(
            plan.config.scdaemon,
            Some("/usr/local/libexec/scdaemon".to_string())
        );
        assert!(plan
            .gpg_agent_conf_lines
            .contains(&"pinentry-program /usr/local/bin/pinentry".to_string()));
        assert!(plan
            .gpg_agent_conf_lines
            .contains(&"scdaemon-program /usr/local/bin/yubikey-notifier".to_string()));
    }

    #[test]
    fn plan_setup_replaces_existing_scdaemon_program() {
        let plan = plan_setup(
            "/new/path",
            Some("/usr/local/libexec/scdaemon".to_string()),
            Some("scdaemon-program /old/path\nother-option foo\n"),
            "Funk.aiff",
            "1.0",
        )
        .unwrap();

        let scd_lines: Vec<_> = plan
            .gpg_agent_conf_lines
            .iter()
            .filter(|l| l.starts_with("scdaemon-program"))
            .collect();
        assert_eq!(scd_lines.len(), 1);
        assert_eq!(scd_lines[0], "scdaemon-program /new/path");
    }

    #[test]
    fn plan_setup_self_reference() {
        let result = plan_setup(
            "/my/binary",
            Some("/my/binary".to_string()),
            None,
            "Funk.aiff",
            "1.0",
        );
        assert!(matches!(result, Err(SetupError::SelfReference)));
    }

    #[test]
    fn plan_setup_no_scdaemon() {
        let result = plan_setup("/my/binary", None, None, "Funk.aiff", "1.0");
        assert!(matches!(result, Err(SetupError::ScdaemonNotFound)));
    }

    #[test]
    fn plan_uninstall_removes_scdaemon_program() {
        let lines = plan_uninstall("pinentry-program /bin/pinentry\nscdaemon-program /my/binary\nother stuff\n");
        assert_eq!(lines.len(), 2);
        assert!(!lines.iter().any(|l| l.contains("scdaemon-program")));
    }
}
