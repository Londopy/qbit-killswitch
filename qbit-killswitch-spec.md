# qbit-killswitch — Software Specification
`SPEC-001` · `v0.4.0` · `Rust 2021` · `Async / Tokio` · `egui`

---

## Overview

A lightweight GUI daemon that monitors the machine's public IP address and automatically
pauses all qBittorrent transfers if the VPN tunnel drops or the IP changes unexpectedly.
Designed for environments where VPN is enforced at the router rather than the host,
making interface-binding impossible. Ships as a self-contained desktop application with
a configurable settings panel, system tray presence, and optional launch-at-startup
registration. Uses the qBittorrent Web API for torrent control and an external IP echo
service for leak detection.

---

## Build & Runtime

| Field           | Value                               |
|-----------------|-------------------------------------|
| Language        | Rust 2021 edition                   |
| Runtime model   | Async, single-process               |
| Async executor  | Tokio (multi-thread)                |
| Binary size     | ~10–15 MB stripped (release)        |
| Build command   | `cargo build --release`             |
| Memory          | < 30 MB RSS at idle (egui overhead) |
| CPU             | Negligible between polls            |

### Target Platforms

| Platform | Target Triple                  | Tray | Startup reg.               |
|----------|--------------------------------|------|----------------------------|
| Windows  | x86_64-pc-windows-msvc         | ✓    | HKCU Run registry          |
| Linux    | x86_64-unknown-linux-gnu       | ✓    | XDG autostart .desktop     |
| macOS    | aarch64-apple-darwin           | ✓    | LaunchAgent plist          |

---

## Dependencies

| Crate          | Version  | Features                        | Role                               |
|----------------|----------|---------------------------------|------------------------------------|
| `tokio`        | 1.x      | `["full"]`                      | Async runtime                      |
| `reqwest`      | 0.12.x   | `["cookies", "rustls-tls"]`     | HTTP client + cookie jar + TLS     |
| `anyhow`       | 1.x      | —                               | Error handling                     |
| `eframe`       | 0.27.x   | `["default"]`                   | egui app framework + window loop   |
| `egui`         | 0.27.x   | —                               | Immediate-mode GUI                 |
| `tray-icon`    | 0.14.x   | —                               | System tray icon + context menu    |
| `auto-launch`  | 0.5.x    | —                               | Cross-platform startup registration|
| `keyring`      | 3.x      | —                               | OS keychain for password storage   |
| `serde`        | 1.x      | `["derive"]`                    | Config serialization               |
| `toml`         | 0.8.x    | —                               | Config file format                 |

> **Note:** `rustls-tls` is required for HTTPS requests to `ip_check_urls`. On platforms
> where `native-tls` is preferred (e.g. macOS system keychain integration), substitute
> `"native-tls"` for `"rustls-tls"`. Do not use neither — the default reqwest build has
> no TLS support.
>
> **Note:** `keyring` delegates to the platform credential store: Windows Credential
> Manager, macOS Keychain, and Linux Secret Service (libsecret / gnome-keyring / kwallet).
> On Linux headless environments where no keyring daemon is running, `keyring` will return
> an error; the app falls back to the TOML plaintext value and shows a persistent warning
> in the status bar.

---

## Configuration

Config is persisted to a TOML file. Editable via the GUI settings panel.
No recompile required for any setting change.

### File Location

| Platform | Path                                                          |
|----------|---------------------------------------------------------------|
| Windows  | `%APPDATA%\qbit-killswitch\config.toml`                       |
| Linux    | `~/.config/qbit-killswitch/config.toml`                       |
| macOS    | `~/Library/Application Support/qbit-killswitch/config.toml`   |

### Schema

```toml
qbit_url          = "http://127.0.0.1:8080"
qbit_user         = "admin"
qbit_pass         = ""          # leave blank if use_keychain = true
use_keychain      = true
ip_check_urls     = [
  "https://ifconfig.me/ip",
  "https://api.ipify.org",
  "https://checkip.amazonaws.com",
]
poll_secs         = 10
fail_threshold    = 2
launch_at_startup = false
minimize_to_tray  = true
```

> **Migration from v0.3.0:** The old `ip_check_url` (single string) key is still
> accepted on load and is automatically promoted to a one-element `ip_check_urls`
> list. The old key is removed from the file on the next Save.

### Parameters

| Key                  | Type       | Default                                                       | Constraints | Description                                                                                          |
|----------------------|------------|---------------------------------------------------------------|-------------|------------------------------------------------------------------------------------------------------|
| `qbit_url`           | String     | `http://127.0.0.1:8080`                                       | —           | Base URL of qBittorrent WebUI.                                                                       |
| `qbit_user`          | String     | `"admin"`                                                     | —           | WebUI username. Also used as the keychain account name.                                              |
| `qbit_pass`          | String     | `""`                                                          | —           | WebUI password. Blank when `use_keychain = true`. Used as plaintext fallback if keychain unavailable.|
| `use_keychain`       | bool       | `true`                                                        | —           | Store and retrieve `qbit_pass` via the OS keychain. Falls back to `qbit_pass` in TOML on error.     |
| `ip_check_urls`      | \[String\] | `["https://ifconfig.me/ip", "https://api.ipify.org", "https://checkip.amazonaws.com"]` | min 1 entry | Ordered list of IP echo services. Tried in sequence; first success wins. All must fail for `None`.  |
| `poll_secs`          | u64        | `10`                                                          | min: 5      | Interval between IP checks in seconds. Values below 5 are clamped to 5.                             |
| `fail_threshold`     | u32        | `2`                                                           | min: 1      | Consecutive failures before pause is triggered.                                                      |
| `launch_at_startup`  | bool       | `false`                                                       | —           | Register app to launch on user login. Toggle writes/removes OS startup entry.                        |
| `minimize_to_tray`   | bool       | `true`                                                        | —           | Close button minimizes to tray rather than exiting.                                                  |

---

## GUI — Settings Panel

Built with `egui` / `eframe`. Single-window application.

### Layout

```
┌──────────────────────────────────────────────┐
│  qbit-killswitch               [─] [□] [✕]   │
├──────────────────────────────────────────────┤
│  Status: ● NOMINAL   VPN IP: 185.x.x.x       │
│                         [Re-detect VPN IP]   │
├──────────────────────────────────────────────┤
│  qBittorrent                                 │
│   URL    [____________________________]      │
│   User   [____________________________]      │
│   Pass   [••••••••••••••••••••••••••••]      │
│          [✓] Store password in OS keychain   │
│                       [Test Connection]      │
├──────────────────────────────────────────────┤
│  Monitor                                     │
│   IP Check URLs                  [+ Add]    │
│   [https://ifconfig.me/ip           ] [✕]   │
│   [https://api.ipify.org            ] [✕]   │
│   [https://checkip.amazonaws.com    ] [✕]   │
│   Poll interval  [__] seconds                │
│   Fail threshold [__] consecutive            │
├──────────────────────────────────────────────┤
│  Application                                 │
│   [✓] Launch at startup                      │
│   [✓] Minimize to tray on close              │
├──────────────────────────────────────────────┤
│                   [Save]  [Cancel]           │
└──────────────────────────────────────────────┘
```

### Status Indicator

Displayed in the header bar at all times.

| State      | Indicator      | Label                                                    |
|------------|----------------|----------------------------------------------------------|
| `STARTING` | ● grey         | `STARTING — waiting for VPN…`                            |
| `NOMINAL`  | ● green        | `NOMINAL — VPN IP: x.x.x.x`                             |
| `DEGRADED` | ● yellow       | `DEGRADED — streak N/M`                                  |
| `PAUSED`   | ● red          | `PAUSED — torrents halted`                               |
| `RECOVERY` | ● cyan (blink) | `RECOVERING…`                                            |
| `NOMINAL`  | ● green        | `⚠ Keychain unavailable — password stored in plaintext` |

The keychain warning variant replaces the normal NOMINAL label when `use_keychain = true`
but the OS keychain could not be accessed. It persists until the app is restarted with a
working keychain or the user disables `use_keychain`.

### Re-detect VPN IP Button

Appears in the status bar header. Visible in all states.

On click:
1. Fetches the current public IP using the `ip_check_urls` fallback sequence.
2. On success:
   - Updates `vpn_ip` to the new IP in memory (not persisted; recalculated at startup).
   - If state is `PAUSED` or `DEGRADED`: resets `fail_streak = 0`, POSTs resume_all, transitions to `NOMINAL`.
   - If state is `NOMINAL`: updates the displayed IP, resets `fail_streak = 0`.
   - Shows a transient status message: `"VPN IP updated to x.x.x.x"` (fades after 3s).
3. On failure (all URLs unreachable): shows inline error `"Could not reach any IP check service — try again"`. State unchanged.

### Save / Cancel Behavior

- **Save** — validates field values (URL parseable, `poll_secs` ≥ 5, `fail_threshold` ≥ 1),
  writes config to disk, applies new settings to the running monitor loop immediately.
  If validation fails, shows inline errors next to the offending fields; does not write.
- **Cancel** — discards all unsaved field changes and resets the form to the last saved
  values. Does not close the window.
- **Window close with unsaved changes** — if any field differs from the last saved state,
  shows a modal: `"You have unsaved changes. Save before closing?"` with **Save**, **Discard**, and **Cancel** options. If no changes are pending, closes (or minimizes to tray) without prompting.

### OS Keychain Behavior

Controlled by the `[✓] Store password in OS keychain` checkbox in the qBittorrent section.

- **Enabling:** On Save, the password is written to the OS keychain under service name
  `"qbit-killswitch"` and account name equal to the current `qbit_user` value. The
  `qbit_pass` field in the TOML is then set to `""`.
- **Disabling:** On Save, the password is written back into the TOML `qbit_pass` field
  and removed from the keychain.
- **Username change:** If `qbit_user` is changed and saved, the old keychain entry is
  deleted and a new entry is created under the new username.
- **Startup load order:** Read `qbit_user` from TOML → attempt keychain lookup →
  on success use keychain value; on failure use `qbit_pass` from TOML and show warning.
- **Keychain unavailable:** If the keychain write fails on Save, the save is aborted and
  an inline error is shown: `"Keychain unavailable — password was not saved. Disable keychain storage to save in config file."` The TOML is not written.

### Test Connection Button

On click:
1. Reads the password from the keychain if `use_keychain = true`, otherwise from the Pass field.
2. POST to `/api/v2/auth/login` with current URL/User and resolved password (does not save config).
3. Show inline result: `✓ Connected` (green) or `✗ Auth failed` (red).
4. Result clears on next keystroke in any qBittorrent field.

---

## System Tray

Provided by `tray-icon`. App runs in tray when window is closed
(if `minimize_to_tray = true`).

### Tray Icon States

| State      | Icon color     | Tooltip                           |
|------------|----------------|-----------------------------------|
| `STARTING` | Grey           | `qbit-killswitch — STARTING`      |
| `NOMINAL`  | Green          | `qbit-killswitch — NOMINAL`       |
| `DEGRADED` | Yellow         | `qbit-killswitch — DEGRADED`      |
| `PAUSED`   | Red            | `qbit-killswitch — PAUSED`        |
| `RECOVERY` | Cyan (blink)   | `qbit-killswitch — RECOVERING`    |

### Tray Context Menu

```
  Open qbit-killswitch
  ─────────────────────
  Status: NOMINAL
  VPN IP: 185.x.x.x
  ─────────────────────
  Resume all torrents      (greyed out when NOMINAL or STARTING)
  Pause all torrents
  Re-detect VPN IP
  ─────────────────────
  Quit
```

"Re-detect VPN IP" follows the same logic as the GUI button (see Re-detect VPN IP Button).
It is available in all states.

---

## Startup Registration

Managed by `auto-launch`. Toggled by the `launch_at_startup` checkbox in the GUI.
On toggle-on, registers the current binary path. On toggle-off, removes the entry.

| Platform | Mechanism                                                              |
|----------|------------------------------------------------------------------------|
| Windows  | `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`      |
| Linux    | `~/.config/autostart/qbit-killswitch.desktop`                          |
| macOS    | `~/Library/LaunchAgents/com.qbit-killswitch.plist`                     |

---

## State Machine

```
STARTING ──► NOMINAL ──► DEGRADED ──► PAUSED ──► RECOVERY ──► NOMINAL
                ▲                                                  │
                └──────────────────────────────────────────────────┘

STARTING ──► PAUSED   (if VPN not detected at startup after max retries)
```

| State      | Condition                                              | Action                                                    |
|------------|--------------------------------------------------------|-----------------------------------------------------------|
| `STARTING` | App just launched; VPN IP not yet confirmed            | Poll IP; wait for a successful read before entering NOMINAL. Torrents are not touched. |
| `NOMINAL`  | IP matches `vpn_ip` recorded on first successful check | None. Torrents running. `fail_streak = 0`.                |
| `DEGRADED` | IP mismatch or echo service unreachable                | Increment `fail_streak`. Update tray → yellow. No torrent action yet. |
| `PAUSED`   | `fail_streak >= fail_threshold`                        | POST pause all torrents. Tray icon → red.                 |
| `RECOVERY` | VPN IP restored while in PAUSED state                  | POST resume all torrents. On success → NOMINAL. On failure, remain PAUSED and retry next poll. |

### Startup IP Validation

The app enters `STARTING` state on launch and **does not record `vpn_ip` until it
receives a successful IP response**. This prevents the race condition where the app
starts before the VPN tunnel is established and incorrectly records the real IP as
the baseline.

During `STARTING`:
- The monitor polls `ip_check_url` at `poll_secs` intervals.
- Each successful response is treated as a candidate `vpn_ip`. On the first success,
  `vpn_ip` is set and the state transitions to `NOMINAL`.
- Failures during `STARTING` increment a separate `start_fail_streak`. If
  `start_fail_streak >= fail_threshold * 3`, the app transitions to `PAUSED`
  as a precaution (torrents halted until manually reviewed), and logs a warning:
  `"Could not determine VPN IP after N attempts — torrents paused as a precaution."`
- The tray icon and status bar both show `STARTING` (grey) during this phase.

---

## Control Flow

```
startup
  load config from TOML (or write defaults if missing)
  promote legacy ip_check_url → ip_check_urls[0] if present
  clamp poll_secs to max(poll_secs, 5)
  resolve password:
    if use_keychain → attempt keyring::get(service="qbit-killswitch", user=qbit_user)
      ├── success → use keychain value
      └── failure → use qbit_pass from TOML, set keychain_warning = true
  register startup entry if launch_at_startup = true
  login to qBittorrent WebUI
  enter STARTING state → tray icon grey
  launch egui window + tray icon
  spawn monitor task (async, background)

fetch_ip() — shared subroutine
  for each url in ip_check_urls (in order):
    GET url (5s timeout)
    ├── 200 + plain-text body that parses as IP → return Some(ip)  ← first success wins
    └── error / timeout / non-IP body → try next url
  all urls exhausted → return None

monitor loop every poll_secs
  result = fetch_ip()
  │
  ├── [STARTING state]
  │   ├── Some(ip) → record as vpn_ip → transition to NOMINAL → tray green
  │   │              reset start_fail_streak = 0
  │   └── None     → start_fail_streak++
  │                  if start_fail_streak >= fail_threshold * 3
  │                    → POST pause_all (precautionary) → tray red, state PAUSED
  │                    log warning: "Could not determine VPN IP — paused"
  │
  ├── [NOMINAL / DEGRADED state]
  │   ├── Some(ip) == vpn_ip → fail_streak = 0
  │   │                         if state == DEGRADED → state = NOMINAL
  │   │                         tray icon → green
  │   │
  │   ├── Some(ip) != vpn_ip → fail_streak++  (IP changed / possible leak)
  │   │                         state = DEGRADED → tray icon → yellow
  │   │                         if fail_streak >= fail_threshold
  │   │                           → POST pause_all → state = PAUSED → tray red
  │   │
  │   └── None               → fail_streak++  (all IP services unreachable)
  │                             state = DEGRADED → tray icon → yellow
  │                             if fail_streak >= fail_threshold
  │                               → POST pause_all → state = PAUSED → tray red
  │
  └── [PAUSED state]
      ├── enforce pause (always, regardless of IP result):
      │     GET /api/v2/torrents/info?filter=downloading
      │     if any torrents found actively downloading:
      │       POST pause_all (re-enforce)
      │       log info: "Re-paused N torrent(s) resumed outside the daemon"
      │
      ├── Some(ip) == vpn_ip → state = RECOVERY → tray cyan (blink)
      │                         POST resume_all
      │                         ├── success → state = NOMINAL → tray green
      │                         │             fail_streak = 0
      │                         └── failure (403 / network) → attempt re-login
      │                               ├── re-login success → retry resume_all
      │                               └── re-login failure → remain PAUSED
      │                                   log error, retry next poll
      │
      └── Some(ip) != vpn_ip or None → remain PAUSED, no further action

re-detect VPN IP (triggered by GUI button or tray menu item)
  result = fetch_ip()
  ├── Some(ip) → vpn_ip = ip
  │              if state in [PAUSED, DEGRADED]:
  │                fail_streak = 0
  │                POST resume_all → state = NOMINAL → tray green
  │              if state == NOMINAL:
  │                fail_streak = 0
  │              show transient message: "VPN IP updated to x.x.x.x" (3s)
  └── None → show inline error: "Could not reach any IP check service — try again"
              state unchanged

session error handling (any POST to qBittorrent API)
  on 403 Unauthorized:
    attempt POST /api/v2/auth/login with resolved password
    ├── login returns "Ok." → retry original request
    └── login fails        → log error "Re-login failed: <reason>"
                              remain in current state, retry next poll
  on network error / timeout:
    log error, remain in current state, retry next poll
```

---

## qBittorrent API Surface

| Method | Endpoint                        | Payload / Params          | Notes                                                           |
|--------|---------------------------------|---------------------------|-----------------------------------------------------------------|
| POST   | `/api/v2/auth/login`            | `username, password`      | Returns `"Ok."` on success. Cookie stored in jar.              |
| POST   | `/api/v2/torrents/pause`        | `hashes=all`              | Pauses all active torrents.                                     |
| POST   | `/api/v2/torrents/resume`       | `hashes=all`              | Resumes all paused torrents.                                    |
| GET    | `/api/v2/torrents/info`         | `filter=downloading`      | Returns JSON array of torrents currently downloading. Used to detect manual resume in PAUSED state. |
| GET    | *(each url in ip_check_urls)*   | —                         | External. Must return a plain-text IP as the response body.     |

Session cookies are stored in `reqwest`'s cookie jar for the lifetime of the process.
On a `403` response to any torrent API call, the app automatically re-authenticates
once before declaring failure (see Session Error Handling in Control Flow).

---

## Known Limitations

- **Belt-and-suspenders role** — if the router killswitch is functioning correctly,
  internet goes fully down on VPN drop and this tool catches the `None` path. The
  router remains the primary line of defense; this tool guards against partial
  or slow-failure scenarios.

- **Session expiry** — qBittorrent sessions can time out. The app will attempt one
  automatic re-login on `403`. If re-login fails, the failure is logged and the
  monitor retries on the next poll. Prolonged re-login failures will leave torrents
  in whatever state they were last set to.

- **Re-pause window** — the re-pause enforcement check runs once per `poll_secs`. A
  torrent manually resumed in the qBittorrent WebUI can upload for up to `poll_secs`
  seconds before being caught. Reduce `poll_secs` (minimum 5) to shrink this window.

- **Linux headless keychain** — on Linux systems without a running Secret Service daemon
  (e.g. a minimal desktop or SSH session), keychain storage is unavailable. The app
  falls back to TOML plaintext and shows a persistent warning. This is expected behavior
  for headless environments.

- **VPN server change** — Re-detect VPN IP resolves the common case (NordVPN reconnects
  to a different server). If the app is in `STARTING` state when the VPN changes servers,
  no intervention is needed — `vpn_ip` has not been set yet and will be recorded from the
  new server on the first successful poll. The remaining edge case is an automated server
  rotation that occurs while the daemon is running in `NOMINAL` state without user
  awareness; in that scenario Re-detect VPN IP must be triggered manually.

---

## Changelog

| Version | Changes |
|---------|---------|
| v0.4.0  | **Fallback IP check URLs:** replaced singular `ip_check_url` with `ip_check_urls` list; monitor tries each in order and only reports failure if all are unreachable. **Re-detect VPN IP:** added button in GUI status bar and tray context menu; resets `vpn_ip` to current IP and resumes torrents if in PAUSED/DEGRADED state. **OS keychain:** added `keyring` crate and `use_keychain` config flag; password stored in platform credential store with TOML plaintext fallback and status bar warning on keychain failure. **Re-pause enforcement:** PAUSED state now polls `/api/v2/torrents/info?filter=downloading` each cycle and re-pauses any torrents resumed outside the daemon. |
| v0.3.0  | Added `STARTING` state and startup IP validation to prevent race condition on boot-time launch. Aligned control flow with state machine (DEGRADED tray icon, RECOVERY path). Added session re-login on 403. Added `rustls-tls` to reqwest features. Added `poll_secs` minimum constraint. Defined Save/Cancel/unsaved-changes behavior. Added RECOVERY and STARTING to tray icon table. |
| v0.2.0  | Initial working draft. |

---

*qbit-killswitch · SPEC-001 · v0.4.0 · Rust 2021 · egui / eframe · NordVPN / Router VPN*
