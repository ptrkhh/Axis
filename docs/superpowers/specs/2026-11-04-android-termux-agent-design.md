# Axis on Android via a Termux agent — design

Date: 2026-11-04
Status: approved

## Intent

Turn an Android tablet into a development-laptop replacement. Code lives in
Termux's home folder and is edited and built with Termux tools; Axis should be
the git GUI there, just as it is on the desktop.

The work is built first as a working app, then proposed to the upstream
maintainer (Choochmeque/Axis) for mainline. It must therefore meet mainline
quality bars from day one: AGENTS.md rules, 97%+ coverage, clippy clean, and
zero behavior change on desktop.

## Why an agent inside Termux

An Android app cannot use Termux's files as real paths:

- Storage Access Framework only yields `content://com.termux.documents/...`
  URIs. libgit2, the `git` CLI (used for ~100 operations in
  `git_cli_service.rs`) and `notify` all need POSIX paths. Rejected.
- `android:sharedUserId="com.termux"` requires signing with Termux's key. The
  supported Termux build is F-Droid's, signed with F-Droid's private key.
  Rejected.
- proot / Termux:X11 running the Linux desktop build. Rejected by the user.
- Termux `RUN_COMMAND` intents: output truncated to ~100KB, one intent per
  call. Rejected.

So the Axis backend runs **inside Termux as Termux's own user** (the "agent"),
and a thin Tauri Android app shows the existing UI and talks to it over
loopback.

## Goals

- Axis's existing React UI, unchanged layout, running in a Tauri Android app on
  a tablet with keyboard and trackpad/mouse.
- All git features work against repos under Termux `$HOME`, using Termux's
  `git`, `git-lfs`, `gpg`, `ssh` when installed.
- First-run and update flow without the user building anything: the app
  carries the agent binary and shows a one-time copy-paste script.
- Desktop builds behave exactly as before. Existing entries in the generated
  `src/bindings/api.ts` stay identical; new commands (avatar bytes) may be
  added to it.

## Non-goals

- Phone-sized or touch-first layout. No layout changes unless something errors.
- Play Store distribution. Termux from Google Play is not supported.
- Remote/headless Axis on desktop (the agent may enable it later; not in scope).
- iOS.
- A clickable folder browser (path input is enough for now).
- In-app prompts for GPG passphrases or HTTPS credentials. The agent has no
  terminal or GUI to prompt with; the user configures Termux once instead
  (e.g. `git config --global credential.helper store`, a gpg-agent passphrase
  cache or loopback pinentry). The setup dialog links to these steps. SSH
  passphrases keep working through Axis's existing in-app `SSH_ASKPASS` path.

## Anti-goals (fail even if it technically works)

- Keeping the device awake: **never** call `termux-wake-lock` or otherwise
  hold wake locks.
- An unauthenticated agent on loopback. Any app on the device can reach
  `127.0.0.1`, and the agent runs hooks and custom-action shell commands.
- A diff the maintainer cannot review: hand-duplicated wrappers per command,
  or desktop behavior changes mixed into the refactor.

## Constraints

- Platforms: Linux, macOS, Windows (desktop, unchanged) + Android (new).
- Only Termux from F-Droid. Termux targets SDK 28 and supports API 24+.
- `pnpm` only. Rust 1.90+. AGENTS.md coding standards (no `unwrap()` in
  production, `use` at top, `"{var}"` format strings, logging, never fail
  silently, i18n for new strings).
- Tauri on target `aarch64-linux-android` compiles in mobile mode (JNI, Android
  activity). The **agent binary must not link Tauri**.
- The agent is built for `aarch64-linux-android` with the NDK, not musl: musl's
  resolver needs `/etc/resolv.conf`, which does not exist on Android, and
  `git2` fetch/push/clone plus the GitHub/GitLab/AI clients need DNS.

## Findings from the spike (2026-11-04)

A throwaway binary using the backend's network/storage crates was built with
NDK 27.2 (`aarch64-linux-android24-clang`) and run on a Samsung XCover7,
Android 16 / SDK 36, as the `adb shell` user:

| Check | Result |
|---|---|
| `git2` open, status, fetch, HTTPS clone | OK (after CA fix below) |
| `reqwest` HTTPS (rustls) + DNS | OK |
| `turso` local DB write | OK |
| `notify` recursive watch | OK |

- Build needs, for `cfg(target_os = "android")` only:
  `openssl-sys = { features = ["vendored"] }` and
  `libssh2-sys = { features = ["vendored-openssl"] }`.
- **CA certificates:** `openssl-src` configures Android with `no-stdio`, so
  OpenSSL cannot load CA files or dirs (`SSL_CERT_FILE`, `SSL_CERT_DIR` and
  `git2::opts::set_ssl_cert_file` all fail). Fix that worked: at agent startup,
  read PEMs from `/apex/com.android.conscrypt/cacerts` (fallback
  `/system/etc/security/cacerts`), parse them in memory with
  `openssl::x509::X509::stack_from_pem`, and register each with
  `libgit2_sys::git_libgit2_opts(GIT_OPT_ADD_SSL_X509_CERT, cert_ptr)` (after
  libgit2 is initialised). `libgit2-sys` 0.18.5 does not export
  `GIT_OPT_ADD_SSL_X509_CERT`; the agent defines a local constant `45` (the
  value in libgit2's `git2/common.h`, one past
  `GIT_OPT_GET_USER_AGENT_PRODUCT` = 44) and must re-check it whenever
  `libgit2-sys` is upgraded. 120 certs loaded; clone and fetch then
  succeeded.
- **Not yet verified:** running inside Termux's own user (Termux `$HOME`,
  Termux `git` on `PATH`, SSH keys / `ssh-agent`). Verifying this on device is
  the first build step. Include the `#!/bin/sh` scripts Axis writes (the
  `SSH_ASKPASS` script in `git_cli_service.rs`, hook templates in
  `models/hooks.rs`): Android has no `/bin/sh`, so they rely on the agent
  inheriting Termux's `LD_PRELOAD` (termux-exec), `PATH` and `TMPDIR`.

### Termux-user verification (2026-11-14)

Closes the "Not yet verified" item above. The same crates were built natively
inside Termux (host `aarch64-linux-android`, same vendored-OpenSSL features)
and run as the Termux user (uid `u0_a341`) on a Samsung SM-X810, Android 16,
from the `Axis` checkout, in two launch modes: direct (`./termux-check`, from
a non-login shell that inherited Termux's environment) and agent-style
(`nohup ./termux-check > ~/check.log 2>&1 &`).

| Check | Direct | `nohup` |
|---|---|---|
| 1 CA registration (120 certs from `/apex/com.android.conscrypt/cacerts`), `git2` open + status of `$PWD` | OK | OK |
| 1b `git2` HTTPS connect to a public repo (TLS verified with the registered CAs) | OK | OK |
| 2 `git2` fetch over SSH (`git@github.com:...`; no agent, fell back to `~/.ssh/id_rsa`) | OK | OK |
| 3 `git --version` via inherited `PATH` (Termux git 2.56.0) | OK | OK |
| 4 `#!/bin/sh` script written to `$TMPDIR`, run with `std::process::Command` | OK (`hook-ok`) | OK (`hook-ok`) |
| 5 env seen by the process | `LD_PRELOAD=$PREFIX/lib/libtermux-exec-ld-preload.so`, `PATH` starts with `$PREFIX/bin`, `TMPDIR=$PREFIX/tmp`, `HOME=/data/data/com.termux/files/home`, `SSH_AUTH_SOCK` unset | same |
| 6a `notify` recursive watch of `$PWD` sees a probe file | OK | OK |
| 6b `turso` write and read back in `$HOME` | OK | OK |

- `ssh_key_from_agent` is not enough on its own: there is no agent in Termux
  by default, so the credential callback must fall back to the key file. The
  SSH fetch also needs a `certificate_check` callback that accepts the host
  key (as `build_certificate_check_callback` does); without one libgit2
  rejects it with "invalid or unknown remote ssh hostkey".
- Check 4 passes, but not only because of termux-exec: on this device
  `/bin` is a symlink to `/system/bin`, so `#!/bin/sh` resolves to Android's
  own shell (it also passed with `LD_PRELOAD` unset). A `#!/usr/bin/env sh`
  script fails with "bad interpreter" without `LD_PRELOAD` and works with it.
  The agent must still inherit `LD_PRELOAD`, and scripts must not assume
  `/bin/sh` is Termux's `sh`.
- Building vendored OpenSSL in Termux needs `pkg install perl`.

## Architecture

```
┌──────────── Android app (Tauri, thin) ────────────┐        ┌──── Termux (com.termux uid) ────┐
│ React UI (existing)                                │        │ axis-agent (no Tauri)           │
│   commands.* / events.* from api.ts (unchanged)    │        │  AppState, services, DB, watcher│
│        │ Tauri IPC                                 │        │  command dispatch table         │
│ Rust proxy: catch-all invoke handler ─────────────┼── WS ─►│  GET /rpc (WebSocket): mutual   │
│   re-emits agent events as Tauri events ◄─────────┼────────┤   auth, then calls + events     │
│   local: opener, clipboard, notifications,         │ token  │  GET /health (build hash)       │
│          setup/handoff, path input UI              │ never  │  127.0.0.1:<fixed port>         │
└────────────────────────────────────────────────────┘  sent  └─────────────────────────────────┘
```

### Frontend: transport stays unchanged

All 248 commands go through `TAURI_INVOKE` and all events through
`__makeEvents__` in the generated `src/bindings/api.ts`. On Android, the Rust
side of the app installs a **catch-all invoke handler**: any command it does not
own locally is forwarded to the agent as `{id, command, args}` JSON, and the
response or error is returned unchanged (`get_file_blob` as a binary frame).
Calls, results and events share one WebSocket; a background task holds it and
re-emits each event under the same name with `app.emit`, so the existing
listeners fire unchanged. If the socket drops, pending calls fail with an
error the frontend already shows, and nothing is sent until a new handshake
succeeds (see Agent runtime).

Frontend changes are limited to:

1. **Path dialog** replacing `@tauri-apps/plugin-dialog` `open()` in the 8
   call sites (WelcomeView, Clone, Init, AddWorktree, Archive, Patch,
   SshKeysSettings, useMenuActions) on Android only: a plain text field
   starting at `~`, no completion. One module exports `open()` with the
   plugin's signature (native dialog on desktop, text field on Android), so
   the 8 call sites change only their import line.
   Platform detection reuses the existing browser-side pattern
   (`navigator.platform` in `useCustomActionShortcuts.ts`): Android is
   `navigator.userAgent` containing `Android`. No backend command.
2. **Hide on Android:** open-in-new-window (`RecentRepoContextMenu`), updater
   UI, "Show in folder", "Open terminal", and anything driven only by the
   native menu. Main shortcuts already work through `react-hotkeys-hook` in
   `useKeyboardShortcuts`; menu-only shortcuts are lost on Android, which is
   accepted.
3. **Setup dialog** (below).
4. **Avatars:** `convertFileSrc` points at the agent's cache, which the app
   cannot read. Serve the bytes through the agent instead.

### Backend: separating commands from Tauri

One declaration per command generates both:

- on desktop, the existing `#[tauri::command] #[specta::specta]` function, so
  `api.ts` and desktop behavior are identical;
- in the agent, an entry in a name → handler dispatch table that deserialises
  the args, calls the same body with `&AppState`, and serialises the result
  (errors serialised the same way Tauri does today, so the frontend error
  handling is unchanged).

Today commands take `State<'_, AppState>` and some take `AppHandle`. 13 files
use `AppHandle` (state, menu, custom_actions, integrations, repository, update,
progress_emitter, background_fetch, file_watcher, git_service, hook_progress,
integrations/service, github/oauth) with 22 `emit` sites. These are replaced
by a small host interface providing what services actually need: emit an
event, resolve the app data dir, open a URL. The Tauri implementation wraps
`AppHandle`; the agent implementation broadcasts over the WebSocket and uses
an agent data dir in Termux. "Open URL" in the agent becomes an event that the
Android app handles with the opener plugin (used by GitHub OAuth; its
loopback callback server keeps working because it runs in the agent on
`127.0.0.1`).

Tauri becomes optional for the backend code: a cargo feature gates Tauri, the
Tauri plugins, menu, tauri-specta events and the command wrappers. The agent
is a separate binary built with that feature off.

The Android app and the agent are both `target_os = "android"`, so
`cfg(target_os)` cannot tell them apart; the Tauri feature does. On Android
the app's `run()` mounts only the proxy and its local commands. It does not
build `AppState`, start watchers or background fetch, or register the
generated command handlers; otherwise those commands would run inside the
app's sandbox, where Termux's repos do not exist.

### Agent runtime

- Binds `127.0.0.1` on a fixed port. `/health` is unauthenticated and returns
  only the build hash. Everything else runs over one WebSocket that starts
  with a mutual challenge-response over fixed-length (32-byte) random nonces,
  so concatenation is unambiguous: the app sends `nonce_app`; the agent
  replies with its own `nonce_agent` and
  `HMAC-SHA256(token, "axis-agent" ‖ nonce_app ‖ nonce_agent)`; the app
  verifies it and answers with
  `HMAC-SHA256(token, "axis-app" ‖ nonce_app ‖ nonce_agent)`. Both sides
  compare in constant time. Use the `hmac` crate (its `verify_slice` is
  constant-time) at the version that pairs with the direct `sha2` 0.11
  dependency; no hand-rolled HMAC. The role label and both nonces are bound into
  each MAC so an attacker cannot reflect the agent's own challenge back at it
  on a second connection to obtain a valid answer. The token itself is never
  sent. Reason: any app can bind
  the fixed port while the agent is down (reboot, phantom process killer); a
  bearer token would be handed to that app, which could then run hooks and
  custom actions as Termux. A squatter learns nothing from the handshake, and
  the app treats a failed handshake as "agent not set up" (setup dialog).
  Accepted risk: a squatter can still deny service (the agent cannot bind);
  the agent logs the bind error so the user can see it in its log file.
- On startup:
  - register the Android CA certs with libgit2 (spike fix);
  - set `PATH` to include `$PREFIX/bin`, so Termux's `git`, `git-lfs`, `gpg`
    and `ssh-keygen` are found;
  - put its database and caches under Termux `$HOME` (exact location is the
    builder's call).
- Reports a build hash (SHA-256 of `/proc/self/exe`) in `/health` so the app
  can detect a stale agent.
- **Idle when the app is not in the foreground:** a connected socket does not
  mean the app is visible (a backgrounded or frozen app keeps its socket). The
  app sends a foreground/background message on lifecycle changes, before
  Android can freeze it; if the app is killed, the kernel closes its socket.
  No ping timeout is needed. While no foreground client
  exists, background fetch and all `notify` watchers are paused; in-flight
  calls still finish and reply. The foreground signal's source is the
  builder's call: Tauri's Android `RunEvent`s if they report both pause and
  resume, otherwise the WebView's `visibilitychange` event calling a local
  app command. On resume the agent emits the existing
  per-repo dirty event (as for inactive repos today), so the UI refreshes
  changes made in Termux meanwhile. The process itself stays up (it costs
  nothing while idle).
- No wake lock. While Axis is in the foreground, Termux's foreground
  notification keeps the agent alive. Android 12+'s phantom process killer
  can still kill it; the setup dialog links to Termux's guidance on disabling
  it.

### Setup and update flow (modelled on claude-dashboard)

Reference: `D:\git\claude-dashboard` (`crates/tauri-app/src/lib.rs`,
`public/app.js` `showSetup`/`checkAgent`, `scripts/release.sh`).

- The APK embeds the agent binary (`include_bytes!` via a build-time env var).
- On launch the app calls `/health`. If the build hash differs from the
  embedded one, the handshake fails, or there is no answer and the app has no
  stored token, it shows a dialog with a one-time script, a Copy button (falls
  back to "long-press to copy") and Try again.
- If there is no answer but the app has a stored token, the agent is most
  likely installed but not running (reboot, Termux closed, phantom process
  killer). The dialog then leads with a one-line start command (the same
  guard the script adds to `~/.bashrc`) to paste into Termux, and keeps the
  full script below it for repair.
- The app generates the auth token. The script carries only a one-time
  handoff code, so the token never passes through the clipboard (readable by
  the keyboard app). The pasted text runs everything in a subshell
  (`sh -e <<'EOF' … EOF` or `( set -e; … )`): pasted lines otherwise run in
  the user's interactive shell, where `set -e` would close the Termux session
  on the first failure and lose the error output. The script:
  1. Downloads the binary and the token from a temporary loopback server the
     app starts for the handoff on an OS-assigned port, presenting the
     one-time code (`curl` into `~/axis-agent.new`, `chmod +x`, `mv -f` over
     the old one). The server closes after one successful handoff.
  2. Writes the token to a file only Termux can read.
  3. Stops any running old agent.
  4. Adds a guard to `~/.bashrc` once: if `/health` fails, start the agent
     with `nohup` and log to a file. No `termux-wake-lock`.
  5. Runs the guard line directly to start the agent now. It does not source
     `~/.bashrc`, so failures in the user's own bashrc cannot abort setup.
- Each setup dialog generates a fresh candidate token. The app adopts it as
  its stored token only after the first successful handshake with it, and
  proves knowledge of the stored token in the handshake on every new
  connection. The script stops at the first failed step (`set -e`), so a
  failed download leaves the old agent and token untouched.
- Accepted residual risk: the one-time handoff code passes through the
  clipboard, which on Android 10+ only the foreground app and the default
  keyboard can read in the background. While no real agent holds the fixed
  port (first setup, or the agent is down), a reader can race the script:
  fetch the token from the handoff server, bind the port and answer the
  handshake. The app would then adopt that token and talk to a fake agent,
  which sees repo paths, commit messages and any secrets entered in settings.
  The app cannot tell the peer apart: since Android 10 apps cannot read
  `/proc/net/tcp` to learn the peer UID, and SELinux per-app categories block
  connecting to another app's Unix socket, so `SO_PEERCRED` is not available
  either. The risk is accepted because the only realistic attacker is the
  default keyboard, which already sees everything typed into Axis. The
  script's own handoff then fails ("already used"), which the setup dialog
  surfaces so the user notices.

### Build and release

- The agent is built with the NDK for `aarch64-linux-android` (API 24) and the
  Android-only vendored OpenSSL features.
- `tauri android init` / `tauri android build --apk --target aarch64` for the
  app. No cleartext-traffic patch is needed while all agent traffic goes
  through the Rust proxy: Android's cleartext policy applies to platform HTTP
  stacks and the WebView, not raw sockets. Add it only if the WebView ever
  loads an agent URL directly. An Android capability file is added; desktop
  capabilities keep their platform filters.
- Termux's support policy is aarch64 first; other ABIs are optional.

## Delivery shape

Built on one branch, structured so it can later be split into reviewable PRs:

1. Separate commands/events/services from Tauri. No behavior change; desktop
   `api.ts` identical.
2. Agent binary: WebSocket transport, handshake auth, CA fix, Termux env.
3. Android app: proxy invoke handler, event relay, setup dialog, path dialog,
   hidden desktop-only features, build scripts.

## Decided by the user

- Thin Tauri Android app plus a one-time setup script, following
  claude-dashboard.
- F-Droid Termux only. No proot. No wake lock.
- Minimal changes: no layout changes unless something errors.
- Path input dialog instead of a folder browser, without completion.
- The Android feature set above, including what is hidden.
- Aim for mainline; build first, propose after.
- GPG/HTTPS credentials are pre-configured in Termux; no in-app prompts.

## Calls made by the designer (open to override)

- Agent target `aarch64-linux-android` via NDK (not musl), for DNS.
- One-declaration macro for commands rather than hand-written wrappers or a
  proc-macro crate.
- Catch-all proxy in the Android app so the frontend transport is untouched.
- Shared token generated by the app, delivered through the one-time handoff,
  and proven by a role-bound HMAC handshake over both nonces; it is never
  sent on the wire.
- One WebSocket for calls and events (not HTTP plus a WebSocket), so the agent
  is authenticated once per connection and a reconnect cannot silently reach
  a port squatter.
- Agent pauses background fetch and file watching while the app is not in
  the foreground, following the battery anti-goal.
- Restarting a stopped agent is a pasted one-line command. Starting it from
  the app via Termux `RUN_COMMAND` (needs `allow-external-apps` and a runtime
  permission) is a later upgrade if pasting proves annoying.
- Release wiring is out of scope until the maintainer is on board: a local
  build script produces a debug-signed APK with the agent embedded. Upstream
  CI publishing, keystore secrets and release assets are proposed later.
- On-device checks under the Termux user are manual handoffs: `adb` cannot
  run as Termux's user because F-Droid Termux is not debuggable. `adb`
  (wireless, device seen: Samsung XCover7, Android 16) can still install the
  APK and run checks as the shell user.
- Same crate with a Tauri feature flag, rather than splitting into new crates,
  to keep the diff readable.

## Known code issues found during review (builder notes)

- `get_file_blob` returns raw bytes; the proxy must pass bytes through, not
  only JSON.
- Some services branch on `cfg(target_os = "linux")` (e.g. `find_gpg_program`
  in `signing_service.rs`); Android's `target_os` is `"android"`, so those
  branches need to include it.
- `lib.rs` uses the single-instance and updater plugins unconditionally, but
  `Cargo.toml` only pulls them in for desktop targets.
- `build.rs` always runs `tauri_build`; the agent build must not.
- CI runs `--all-features`, so any new cargo feature must be additive (a
  feature that turns Tauri *on* is fine; one that turns it *off* is not).
- Build-time secrets from `.env` (`dotenvy_macro`, e.g. GitHub OAuth) are
  needed by the agent build too.

## Left to the builder

- Port number, URL shapes, message framing, token file location, agent data
  dir.
- Exact macro syntax and the host interface's method set.
- How the APK embeds the agent and how CI builds the APK.
- Test strategy to keep 97% coverage across feature combinations.
