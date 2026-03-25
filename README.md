# yubikey-notifier

A macOS tool that plays a sound and shows a notification when your YubiKey is waiting for a touch.

Works by acting as a transparent scdaemon proxy -- gpg-agent launches it instead of the real scdaemon, and it intercepts the protocol to detect when a touch-requiring operation (PKSIGN, PKDECRYPT, PKAUTH) takes longer than 1 second to complete.

## Install

```bash
# Download the latest release
curl -fSL https://github.com/bdtfs/yubikey-notifier/releases/latest/download/yubikey-notifier -o /usr/local/bin/yubikey-notifier
chmod +x /usr/local/bin/yubikey-notifier

# Configure as scdaemon wrapper
yubikey-notifier --setup
```

That's it. The setup command:
1. Detects your real scdaemon path via `gpgconf`
2. Saves it to `~/.config/yubikey-notifier/config`
3. Adds `scdaemon-program /usr/local/bin/yubikey-notifier` to `~/.gnupg/gpg-agent.conf`
4. Restarts gpg-agent

No background service or LaunchAgent needed -- gpg-agent spawns the wrapper on demand.

### Build from source

```bash
git clone https://github.com/bdtfs/yubikey-notifier.git
cd yubikey-notifier
cargo build --release
cp target/release/yubikey-notifier /usr/local/bin/
yubikey-notifier --setup
```

## Uninstall

```bash
yubikey-notifier --uninstall
rm /usr/local/bin/yubikey-notifier
```

## How it works

```
gpg-agent
  |
  +-- yubikey-notifier (scdaemon wrapper)
        |
        +-- real scdaemon
              |
              +-- YubiKey (via PC/SC)
```

1. gpg-agent sends a command like `PKSIGN --hash=sha512` through the wrapper
2. The wrapper forwards it to the real scdaemon and starts a 1-second grace timer
3. If scdaemon responds with `OK` within 1 second, no alert fires (touch wasn't needed or was already provided)
4. If 1 second passes with no response, a macOS notification and looping sound alert the user to touch
5. On completion (`OK`), a success sound (Glass) plays; on error/timeout (`ERR`), an error sound (Basso) plays
6. Binary data from scdaemon (key grips, signatures) passes through transparently

## Options

```
--setup            Configure as scdaemon wrapper for gpg-agent
--uninstall        Remove scdaemon wrapper configuration
--sound <NAME>     Alert sound (default: Funk)
--volume <FLOAT>   Volume multiplier, 1.0 = normal (default: 2.0)
--list-sounds      List available macOS system sounds
--help             Show help
```

### Sounds

Alert: Basso, Blow, Bottle, Frog, **Funk** (default), Glass, Hero, Morse, Ping, Pop, Purr, Sosumi, Submarine, Tink

Completion: Glass (success), Basso (error/timeout)

## What it covers

| Operation | Detected? |
|-----------|-----------|
| GPG signing (`gpg --sign`, `git commit -S`) | Yes |
| GPG decryption | Yes |
| SSH auth via GPG smartcard (PKAUTH) | Yes |
| FIDO2/WebAuthn in browsers | No (macOS shows its own dialog) |

## Requirements

- macOS (uses `afplay` and `osascript`)
- GnuPG with scdaemon
- A YubiKey with OpenPGP smart card interface

## License

MIT
