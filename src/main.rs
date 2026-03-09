use std::env;
use std::fs;
use std::io::{self, BufReader};
use std::process::{self, Command, Stdio};
use std::sync::Arc;
use std::thread;

use yubikey_notifier::alert::MacAlerter;
use yubikey_notifier::config::Config;
use yubikey_notifier::event::{self, Event};
use yubikey_notifier::proxy;
use yubikey_notifier::setup;
use yubikey_notifier::sound;

fn print_usage() {
    eprintln!("Usage: yubikey-notifier [OPTIONS]\n");
    eprintln!("Options:");
    eprintln!("  --setup            Configure as scdaemon wrapper for gpg-agent");
    eprintln!("  --uninstall        Remove scdaemon wrapper configuration");
    eprintln!("  --sound <NAME>     Alert sound (default: Funk)");
    eprintln!("  --volume <FLOAT>   Volume multiplier, 1.0 = normal (default: 2.0)");
    eprintln!("  --list-sounds      List available macOS system sounds");
    eprintln!("  --help             Show this help");
}

fn run_wrapper(args: &[String]) {
    let config = Config::load();
    let real_scdaemon = setup::get_real_scdaemon(&config);

    let (event_sink, event_rx) = event::channel();

    // Spawn debug event logger
    let debug_mode = cfg!(debug_assertions) || env::var("YUBIKEY_DEBUG").is_ok();
    thread::spawn(move || {
        for event in event_rx {
            match &event {
                Event::ProxyStarted => eprintln!("[debug] proxy started"),
                Event::ProxyFinished { exit_code } => {
                    eprintln!("[debug] proxy finished (exit_code={exit_code})")
                }
                Event::TouchRequired { command } => {
                    eprintln!("[debug] touch required: {command}")
                }
                Event::TouchCompleted { success } => {
                    eprintln!("[debug] touch completed (success={success})")
                }
                Event::AlertStarted => eprintln!("[debug] alert started"),
                Event::AlertStopped => eprintln!("[debug] alert stopped"),
                Event::LineForwarded { direction, line } => {
                    if debug_mode {
                        eprintln!("[debug] {direction} {line}");
                    }
                }
            }
        }
    });

    let alerter: Arc<dyn yubikey_notifier::alert::Alerter> = Arc::new(MacAlerter {
        sound: config.sound.clone(),
        volume: config.volume.clone(),
        events: event_sink.clone(),
    });

    // Spawn real scdaemon
    let mut child = match Command::new(&real_scdaemon)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERR failed to spawn {}: {}", real_scdaemon, e);
            process::exit(1);
        }
    };

    let child_stdin = child.stdin.take().expect("child stdin");
    let child_stdout = child.stdout.take().expect("child stdout");

    // Run the proxy
    proxy::run_proxy(
        BufReader::new(io::stdin()),
        io::stdout(),
        BufReader::new(child_stdout),
        child_stdin,
        alerter,
        event_sink,
    );

    // Wait for child and exit with its status
    let status = child.wait().unwrap_or_else(|_| process::exit(1));
    process::exit(status.code().unwrap_or(1));
}

fn cmd_setup(sound_name: &str, volume: &str) {
    let my_path = env::current_exe().unwrap_or_else(|e| {
        eprintln!("Error: cannot determine own path: {e}");
        process::exit(1);
    });
    let my_path = my_path.to_string_lossy().to_string();

    let detected = setup::detect_real_scdaemon();
    let existing_conf = fs::read_to_string(setup::gpg_agent_conf_path()).ok();

    let plan = match setup::plan_setup(
        &my_path,
        detected,
        existing_conf.as_deref(),
        sound_name,
        volume,
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    if let Some(ref scd) = plan.config.scdaemon {
        eprintln!("Detected real scdaemon: {scd}");
    }

    if let Err(e) = setup::execute_setup(
        &plan,
        &yubikey_notifier::config::default_config_path(),
        &setup::gpg_agent_conf_path(),
    ) {
        eprintln!("Error: {e}");
        process::exit(1);
    }

    eprintln!("\nSetup complete! YubiKey touch notifications are now active.");
    eprintln!("Test with: echo test | gpg --sign > /dev/null");
}

fn cmd_uninstall() {
    if let Err(e) = setup::execute_uninstall(
        &setup::gpg_agent_conf_path(),
        &yubikey_notifier::config::default_config_path(),
    ) {
        eprintln!("Error: {e}");
        process::exit(1);
    }
    eprintln!("\nUninstall complete. Original scdaemon restored.");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Wrapper mode: if --multi-server is in args, we're being called by gpg-agent
    if args.iter().any(|a| a == "--multi-server") {
        run_wrapper(&args[1..]);
        return;
    }

    // CLI mode
    let mut sound_name = sound::DEFAULT_SOUND.to_string();
    let mut volume = sound::DEFAULT_VOLUME.to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                return;
            }
            "--list-sounds" => {
                sound::list_sounds();
                return;
            }
            "--setup" => {
                // Parse remaining args for --sound/--volume
                let mut j = i + 1;
                while j < args.len() {
                    match args[j].as_str() {
                        "--sound" => {
                            j += 1;
                            if let Some(s) = args.get(j) {
                                sound_name = sound::resolve_sound(s);
                            }
                        }
                        "--volume" => {
                            j += 1;
                            if let Some(v) = args.get(j) {
                                volume = v.clone();
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                cmd_setup(&sound_name, &volume);
                return;
            }
            "--uninstall" => {
                cmd_uninstall();
                return;
            }
            "--sound" => {
                i += 1;
                sound_name =
                    sound::resolve_sound(args.get(i).map(|s| s.as_str()).unwrap_or("Funk"));
            }
            "--volume" => {
                i += 1;
                volume = args
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| sound::DEFAULT_VOLUME.to_string());
            }
            other => {
                eprintln!("Unknown option: {other}");
                print_usage();
                process::exit(1);
            }
        }
        i += 1;
    }

    print_usage();
}
