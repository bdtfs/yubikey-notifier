# yubikey-notifier

A tiny macOS daemon that plays a sound and shows a notification when your YubiKey is waiting for a touch.

If your YubiKey is tucked away where you can't see it blinking, this tool makes sure you never miss a touch prompt during GPG signing, SSH authentication, or other smart card operations.

## How it works

1. **Monitors PC/SC** — polls the smart card interface to detect when another process (like `gpg-agent`) grabs exclusive access to the YubiKey
2. **Checks for signing processes** — only alerts when a `gpg`, `gpg2`, or `ssh` process is actually running (ignores background `gpg-agent` housekeeping)
3. **Plays sound + notification** — after a 1-second delay (so the YubiKey has time to start blinking), plays a looping alert sound and shows a macOS notification
4. **Stops instantly on touch** — detects when the signing process exits and kills the sound immediately

## Install

```bash
# Clone and build
git clone https://github.com/bdtfs/yubikey-notifier.git
cd yubikey-notifier
cargo build --release

# Copy to PATH
cp target/release/yubikey-notifier /usr/local/bin/
```

## Run

```bash
# Foreground
yubikey-notifier

# With custom sound and volume
yubikey-notifier --sound Hero --volume 3.0

# List available sounds
yubikey-notifier --list-sounds
```

## Run as a background service (recommended)

Create a LaunchAgent so it starts automatically on login:

```bash
cat > ~/Library/LaunchAgents/com.yubikey-notifier.plist << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.yubikey-notifier</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/yubikey-notifier</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>/tmp/yubikey-notifier.log</string>
</dict>
</plist>
EOF

# Start it
launchctl load ~/Library/LaunchAgents/com.yubikey-notifier.plist

# Check it's running
launchctl list | grep yubikey
```

### Managing the service

```bash
# Stop
launchctl unload ~/Library/LaunchAgents/com.yubikey-notifier.plist

# Start
launchctl load ~/Library/LaunchAgents/com.yubikey-notifier.plist

# View log
cat /tmp/yubikey-notifier.log
```

## Options

```
--sound <NAME>     Alert sound (default: Funk). Failure always plays Basso.
--volume <FLOAT>   Volume multiplier, 1.0 = normal (default: 2.0)
--list-sounds      List available macOS system sounds
--help             Show help
```

### Available sounds

Basso, Blow, Bottle, Frog, **Funk** (default), Glass, Hero, Morse, Ping, Pop, Purr, Sosumi, Submarine, Tink

## What it covers

| Operation | Detected? | How |
|-----------|-----------|-----|
| GPG signing (`gpg --sign`, `git commit -S`) | Yes | PC/SC + process monitoring |
| SSH with YubiKey (PIV) | Yes | PC/SC + process monitoring |
| FIDO2/WebAuthn in browsers | No | macOS shows its own system dialog for these |

## Requirements

- macOS (uses `afplay`, `osascript`, and the built-in PC/SC framework)
- Rust toolchain (to build)
- A YubiKey with smart card (CCID) interface

## How detection works

The YubiKey exposes a CCID (smart card) interface. When `gpg-agent` initiates a signing operation that requires touch, it grabs exclusive PC/SC access to the card. The notifier detects this via `SCardConnect` returning `SCARD_E_SHARING_VIOLATION`.

To avoid false positives from background `gpg-agent`/`scdaemon` card housekeeping, the notifier also requires a `gpg`, `gpg2`, or `ssh` process to be running before triggering an alert.

When the signing process exits, the alert stops immediately — even though `gpg-agent` may hold the card for several more seconds.

## License

MIT
