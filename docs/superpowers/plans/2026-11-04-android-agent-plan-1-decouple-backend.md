# Android Agent — Plan 1: Decouple the backend from Tauri

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every backend command and service runnable without Tauri, behind an additive `agent` cargo feature, while desktop Axis and the generated `src/bindings/api.ts` stay byte-identical.

**Architecture:** Services stop holding `tauri::AppHandle` and instead hold a small `Host` trait object (emit an event by name, open a URL). Events gain a Tauri-free `AxisEvent::NAME` equal to tauri-specta's name. Each command is declared once with an `axis_command!` macro that emits the existing Tauri command (feature `app`) and a plain `run(&AppState, ..)` function, from which the `agent` feature builds a name → handler dispatch table. Finally Tauri becomes an optional dependency behind the default `app` feature, so `--no-default-features --features agent` compiles with no Tauri in the dependency tree.

**Tech Stack:** Rust 2021 (1.90+), Tauri 2, tauri-specta rc.25, tokio, serde_json, cargo-llvm-cov.

**Spec:** `docs/superpowers/specs/2026-11-04-android-termux-agent-design.md` (sections "Backend: separating commands from Tauri", "Delivery shape" item 1, "Known code issues"). Plans 2 (agent binary and transport) and 3 (Android app) build on the interfaces this plan produces.

## Global Constraints

- Desktop behavior unchanged; `src/bindings/api.ts` byte-identical after `cargo test export_typescript_bindings --manifest-path src-tauri/Cargo.toml`.
- New cargo features are additive: `default = ["app"]`; `app` turns Tauri on; `agent` adds the dispatch table. CI runs `--all-features`, so `app` + `agent` together must compile and pass.
- The `agent` build (`--no-default-features --features agent`) must not link `tauri`, any `tauri-plugin-*`, `tauri-build` or `tauri-specta`.
- AGENTS.md: no `unwrap()` in production (`-D clippy::unwrap_used` in CI), `expect()` only in tests, all `use` at file top, `"foo: {bar}"` format strings, never fail silently (log errors), 97%+ coverage.
- After Rust changes: `cargo clippy --all-targets --all-features --manifest-path src-tauri/Cargo.toml -- -D warnings -D clippy::unwrap_used` and `cargo fmt --manifest-path src-tauri/Cargo.toml`.
- `pnpm` only. Frontend code is not touched by this plan.
- Every task ends with `git diff --exit-code src/bindings/api.ts` after regenerating bindings (from Task 3 on).

## Review Focus

1. **Startup ordering:** a command that needs the host, called before `attach_host`, must return the same error as today's "App handle not set" path (`AxisError::Other`), not panic. Test in Task 3.
2. **Spawning from the Tauri setup thread:** `start_background_fetch` runs inside `.setup()`, outside any tokio context; the runtime shim must not panic there. Test in Task 2.
3. **Argument casing:** agent dispatch receives the same JSON the frontend sends today — camelCase keys (`{"oldName":…,"newName":…}`) with optional keys omitted. Test in Task 5.
4. **Error payload parity:** an agent error must serialize exactly as Tauri serializes `AxisError` (`{"type":"NoRepositoryOpen"}`, `{"type":"GitError","data":"…"}`), or the frontend's error handling breaks. Test in Task 5.
5. **Commit-cache invalidation after HEAD/ref changes** keeps working once the file watcher no longer reaches `AppState` via `try_state`. Test in Task 4.

---

### Task 1: Verify the runtime inside Termux (manual, on device)

The spike proved the crates run as the `adb shell` user. This task proves the Termux-user assumptions the spec marks unverified, before any refactor. Nothing here is committed to the product code.

**Files:**
- Create (outside the repo, throwaway): `$SCRATCH/termux-check/` — a binary crate with the spike's `Cargo.toml` (git2 `https,ssh,cred`; turso 0.7; notify 8; tokio full; reqwest rustls; Android-only `openssl-sys` vendored, `libssh2-sys` vendored-openssl, `openssl`, `foreign-types` 0.3, `libgit2-sys` 0.18) and `.cargo/config.toml` pointing at NDK 27.2 `aarch64-linux-android24-clang` (see spec "Findings from the spike").
- Modify: `docs/superpowers/specs/2026-11-04-android-termux-agent-design.md` (Findings section)

- [ ] **Step 1: Write the check binary.** Each check prints `[ OK ]`/`[FAIL] <error>`:
  1. In-memory CA registration (spec's `GIT_OPT_ADD_SSL_X509_CERT` loop) then `git2` open + status of `$PWD`.
  2. `git2` fetch of `origin` with `Cred::ssh_key` from `~/.ssh/id_ed25519` (or the agent).
  3. `std::process::Command::new("git").arg("--version")` — Termux git found via inherited `PATH`.
  4. Write `$TMPDIR/axis-hook.sh` containing `#!/bin/sh\necho hook-ok`, `chmod 755`, run it with `std::process::Command::new(path)`. Expect stdout `hook-ok` (proves termux-exec rewrites the shebang for processes spawned by Rust's `std::process`, the way `HookService` runs hooks).
  5. Same script run via `git -c core.hooksPath=$TMPDIR` style is not needed; instead print `LD_PRELOAD`, `PATH`, `TMPDIR`, `HOME`, `PREFIX` as seen by the process.
  6. `notify` watch of `$PWD` with a probe file; `turso` DB write in `$HOME/.axis-check.db`.
- [ ] **Step 2: Build** `cargo build --release --target aarch64-linux-android` and `adb push` the binary to `/sdcard/Download/termux-check`.
- [ ] **Step 3: Hand off to the user:** in Termux run `cp ~/storage/downloads/termux-check ~ && chmod +x ~/termux-check && cd ~/<repo-with-ssh-origin> && ~/termux-check`, then the same under `nohup ~/termux-check > ~/check.log 2>&1 &` (the agent's launch mode) and `cat ~/check.log`. Expected: all `[ OK ]` in both runs.
- [ ] **Step 4: Record results** in the spec's Findings table (one row per check, both launch modes). If check 4 fails under Rust spawn, add to the spec's builder notes: "spawn scripts through `$PREFIX/bin/sh <script>` on Android" and stop to tell the user before Task 2.
- [ ] **Step 5: Commit** `git add docs/superpowers/specs/2026-11-04-android-termux-agent-design.md && git commit -m "docs: record Termux on-device verification"`

---

### Task 2: Host trait, event names, runtime shim

**Files:**
- Create: `src-tauri/src/host.rs`, `src-tauri/src/runtime.rs`
- Modify: `src-tauri/src/lib.rs` (declare `pub mod host; pub mod runtime;`), `src-tauri/src/events/mod.rs`, `src-tauri/Cargo.toml` (dev-dependency `tauri = { version = "2", features = ["test"] }`)
- Test: inline `#[cfg(test)] mod tests` in `host.rs`, `runtime.rs`, `events/mod.rs`

**Interfaces:**
- Produces:
  - `pub trait Host: Send + Sync + 'static { fn emit_json(&self, event: &'static str, payload: serde_json::Value); fn open_url(&self, url: &str) -> crate::error::Result<()>; }`
  - `pub type HostHandle = std::sync::Arc<dyn Host>;`
  - `pub trait AxisEvent: serde::Serialize { const NAME: &'static str; }`
  - `pub fn emit<E: AxisEvent>(host: &dyn Host, event: &E)` — serializes with `serde_json::to_value`; on failure logs `log::error!("Failed to serialize event {}: {e}", E::NAME)` and returns.
  - `pub struct TauriHost(pub tauri::AppHandle)` implementing `Host` (`emit_json` → `tauri::Emitter::emit(&self.0, event, payload)`, logging errors; `open_url` → `tauri_plugin_opener::OpenerExt::opener(&self.0).open_url(url, None::<&str>)` mapped to `AxisError::Other`).
  - `#[cfg(any(test, feature = "integration"))] pub struct RecordingHost { pub events: parking_lot::Mutex<Vec<(String, serde_json::Value)>>, pub opened_urls: parking_lot::Mutex<Vec<String>> }` with `RecordingHost::new() -> std::sync::Arc<Self>` and `fn names(&self) -> Vec<String>`.
  - `crate::runtime::{spawn, spawn_blocking, JoinHandle}` — same signatures as `tauri::async_runtime::{spawn, spawn_blocking}`; `JoinHandle<T>` is `tauri::async_runtime::JoinHandle<T>` when `app` is enabled (Task 7 adds the cfg; until then always Tauri's).
  - `AxisEvent` impls for all 13 events via one `macro_rules! axis_events { ($($ty:ty => $name:literal),* $(,)?) => { … } }` in `events/mod.rs`: `FilesChangedEvent => "files-changed-event"`, `GitOperationProgressEvent => "git-operation-progress-event"`, `HeadChangedEvent => "head-changed-event"`, `HookProgressEvent => "hook-progress-event"`, `IndexChangedEvent => "index-changed-event"`, `IntegrationStatusChangedEvent => "integration-status-changed-event"`, `MenuActionEvent => "menu-action-event"`, `OAuthCallbackEvent => "o-auth-callback-event"`, `RefChangedEvent => "ref-changed-event"`, `RemoteFetchedEvent => "remote-fetched-event"`, `RepositoryDirtyEvent => "repository-dirty-event"`, `UpdateDownloadProgressEvent => "update-download-progress-event"`, `WatchErrorEvent => "watch-error-event"`. Copy each name from the `events` mapping in `src/bindings/api.ts`; the test below is the source of truth.

- [ ] **Step 1: Write failing tests**

```rust
// events/mod.rs
#[test]
fn axis_event_names_match_tauri_specta() {
    fn same<E: AxisEvent + tauri_specta::Event>() { assert_eq!(<E as AxisEvent>::NAME, <E as tauri_specta::Event>::NAME); }
    same::<FilesChangedEvent>(); same::<GitOperationProgressEvent>(); same::<HeadChangedEvent>();
    same::<HookProgressEvent>(); same::<IndexChangedEvent>(); same::<IntegrationStatusChangedEvent>();
    same::<MenuActionEvent>(); same::<OAuthCallbackEvent>(); same::<RefChangedEvent>();
    same::<RemoteFetchedEvent>(); same::<RepositoryDirtyEvent>(); same::<UpdateDownloadProgressEvent>();
    same::<WatchErrorEvent>();
}

// host.rs
#[test]
fn emit_records_name_and_payload() {
    let host = RecordingHost::new();
    emit(host.as_ref(), &RefChangedEvent { ref_name: "refs/heads/main".into() });
    let events = host.events.lock();
    assert_eq!(events[0].0, "ref-changed-event");
    assert_eq!(events[0].1, serde_json::json!({ "refName": "refs/heads/main" })); // match the event's serde casing
}

#[test]
fn tauri_host_emits_to_listeners() {
    let app = tauri::test::mock_app();
    // listen on "head-changed-event" via tauri::Listener, emit through TauriHost, assert the handler ran once
}

// runtime.rs
#[test]
fn spawn_works_outside_tokio_context() {
    // plain std thread, no runtime entered, mirrors Tauri's .setup()
    let out = std::thread::spawn(|| { let h = crate::runtime::spawn(async { 7 }); h }).join().expect("thread");
    assert_eq!(tauri::async_runtime::block_on(out).expect("join"), 7);
}
```

- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml --lib host:: runtime:: events::` — Expected: FAIL (modules missing).
- [ ] **Step 3: Implement** `host.rs`, `runtime.rs`, the `axis_events!` impls, and replace the 5 `tauri::async_runtime::{spawn, spawn_blocking}` call sites (`commands/repository.rs:223`, `services/background_fetch.rs:8,37`, `services/git_service.rs:81`, `services/ops/mod.rs:46`) with `crate::runtime::…`.
- [ ] **Step 4: Run** the tests again plus `cargo test --manifest-path src-tauri/Cargo.toml` — Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "refactor: add Host trait, Tauri-free event names and runtime shim"`

---

### Task 3: AppState owns a Host; progress and hook emitters use it

**Files:**
- Modify: `src-tauri/src/state.rs`, `src-tauri/src/services/progress_emitter.rs`, `src-tauri/src/services/hook_progress.rs`, `src-tauri/src/lib.rs` (setup), every command that called `state.get_app_handle()` (`commands/repository.rs:72,105`, others found by `grep -rn get_app_handle src-tauri/src`)
- Test: inline tests in `state.rs`, `progress_emitter.rs`, `hook_progress.rs`

**Interfaces:**
- Consumes: `Host`, `HostHandle`, `emit`, `RecordingHost` (Task 2).
- Produces:
  - `AppState::attach_host(&self, host: HostHandle, app_data_dir: PathBuf)` — replaces `set_app_handle`; also creates `AvatarService::new(&app_data_dir)` as today.
  - `AppState::host(&self) -> Result<HostHandle>` — `Err(AxisError::Other("Host not attached".to_string()))` before `attach_host`.
  - `AppState::app_data_dir(&self) -> Result<PathBuf>` — same error rule.
  - `ProgressContext::new(host: HostHandle, registry: Arc<ProgressRegistry>)`, `ProgressEmitter::new(host: HostHandle, operation_id: &str, cancel_token: Arc<AtomicBool>)`, `HookProgressEmitter::new(host: HostHandle, registry: Arc<ProgressRegistry>)`.
  - `get_app_handle` and `set_app_handle` are removed. `lib.rs` setup calls `app_state.attach_host(Arc::new(TauriHost(app.handle().clone())), app_data_dir.clone())`.

- [ ] **Step 1: Write failing tests**
  - `state.rs`: `host_before_attach_is_error` — `AppState::new(Database::open_in_memory().await…)`; `assert!(matches!(state.host(), Err(AxisError::Other(m)) if m == "Host not attached"))`; same for `app_data_dir()` and `get_git_service()` with a repo path set.
  - `progress_emitter.rs`: `emit_complete_reaches_host` — `ProgressContext::new(RecordingHost::new(), …)`, call `emit_complete(GitOperationType::Fetch)`, assert `host.names() == ["git-operation-progress-event"]`.
  - `hook_progress.rs`: `emit_running_reaches_host` — assert one `"hook-progress-event"`.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml --lib state:: progress_emitter:: hook_progress::` — Expected: FAIL (compile errors on new signatures).
- [ ] **Step 3: Implement** the signatures above; replace each `event.emit(&self.app_handle)` with `crate::host::emit(self.host.as_ref(), &event)`.
- [ ] **Step 4: Run** full `cargo test --manifest-path src-tauri/Cargo.toml` and the bindings export, then `git diff --exit-code src/bindings/api.ts` — Expected: PASS, no diff.
- [ ] **Step 5: Commit** `git commit -m "refactor: route progress and hook events through Host"`

---

### Task 4: File watcher, GitService, background fetch and OAuth without AppHandle

**Files:**
- Modify: `src-tauri/src/services/file_watcher.rs`, `src-tauri/src/services/git_service.rs`, `src-tauri/src/state.rs` (`RepositoryCache::get_or_open`, passphrase cache), `src-tauri/src/services/background_fetch.rs`, `src-tauri/src/services/integrations/service.rs`, `src-tauri/src/services/integrations/github/oauth.rs`, `src-tauri/src/commands/integrations.rs`, `src-tauri/src/commands/repository.rs`
- Test: inline tests in those files

**Interfaces:**
- Consumes: Task 2 and Task 3 interfaces.
- Produces:
  - `#[derive(Clone)] pub struct WatchContext { pub host: HostHandle, pub commit_cache: Arc<CommitCache> }` in `services/file_watcher.rs`; `AppState::watch_context(&self) -> Result<WatchContext>`.
  - `FileWatcher::new(repo_path: PathBuf, ctx: WatchContext, is_active: bool) -> notify::Result<Self>` — commit-cache invalidation uses `ctx.commit_cache` instead of `app_handle.try_state::<AppState>()`.
  - `GitService::open(path: &Path, ctx: WatchContext, is_active: bool)`, `GitService::init(path: &Path, bare: bool, ctx: WatchContext)`, `GitService::clone(url, path, ctx: WatchContext, progress_cb, ssh_credentials)`; `RepositoryCache::get_or_open(&self, path: &Path, ctx: &WatchContext, is_active: bool)`.
  - `pub struct SshPassphraseCache` (moved from the `AppState` field) with `get(&self, key_path: &str) -> Option<SecretString>`, `set(&self, key_path: String, passphrase: SecretString)`, `clear(&self)`; `AppState` keeps its existing passphrase methods delegating to an `Arc<SshPassphraseCache>`.
  - `pub struct FetchContext { pub cache: Arc<RepositoryCache>, pub database: Arc<Database>, pub passphrases: Arc<SshPassphraseCache>, pub host: HostHandle }`; `BackgroundFetchService::start(&self, ctx: FetchContext, interval_minutes: u32)` and `restart(&self, ctx: FetchContext, interval_minutes: u32)`; `AppState::start_background_fetch` / `restart_background_fetch` build it.
  - `GitHubOAuthFlow::start(&self, host: &dyn Host) -> Result<String>` and `IntegrationService::start_oauth(&self, provider: ProviderType, host: &dyn Host) -> Result<()>` — the browser is opened with `host.open_url(&auth_url)`.

- [ ] **Step 1: Write failing tests**
  - `file_watcher.rs`: `head_change_invalidates_commit_cache_and_emits` — temp repo (`tempfile`), `CommitCache` pre-filled for that repo, `FileWatcher::new(path, WatchContext { host: RecordingHost, commit_cache }, true)`, rewrite `.git/HEAD`, poll up to 3 s; assert `"head-changed-event"` recorded and the cache entry is gone.
  - `file_watcher.rs`: `inactive_repo_emits_repository_dirty_only` — `is_active = false`, touch a file, assert names contain `"repository-dirty-event"` and not `"files-changed-event"`.
  - `oauth.rs`: `start_opens_auth_url_via_host` — `RecordingHost`; run `start` in a task, wait until `opened_urls` has one entry starting with `https://github.com/login/oauth/authorize`, then abort the task.
  - `background_fetch.rs`: `start_and_stop_with_fetch_context` — start with interval 1, assert `is_running()`, `stop()`, assert not running.
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml --lib file_watcher:: oauth:: background_fetch::` — Expected: FAIL.
- [ ] **Step 3: Implement** the signatures; `grep -rn "AppHandle\|tauri::" src-tauri/src/services` must return nothing afterwards.
- [ ] **Step 4: Run** full tests, bindings export, `git diff --exit-code src/bindings/api.ts` — Expected: PASS, no diff.
- [ ] **Step 5: Commit** `git commit -m "refactor: remove AppHandle from services"`

---

### Task 5: `axis_command!` macro and agent dispatch, proven on branches

**Files:**
- Create: `src-tauri/src/commands/macros.rs`, `src-tauri/src/agent/mod.rs`, `src-tauri/src/agent/dispatch.rs`
- Modify: `src-tauri/src/commands/mod.rs` (`#[macro_use] mod macros;` first), `src-tauri/src/commands/branches.rs`, `src-tauri/src/lib.rs` (`#[cfg(feature = "agent")] pub mod agent;`), `src-tauri/Cargo.toml` (`[features] agent = []`)
- Test: `src-tauri/src/agent/dispatch.rs` inline tests

**Interfaces:**
- Consumes: `AppState` (Task 3/4).
- Produces:
  - `axis_command!` accepting
    ```rust
    axis_command! {
        /// doc comment
        pub async fn create_branch(state, name: String, options: CreateBranchOptions) -> Result<Branch> { /* body; `state: &AppState` */ }
    }
    ```
    and expanding to (a) `#[tauri::command] #[specta::specta] pub async fn create_branch(state: tauri::State<'_, AppState>, name: String, options: CreateBranchOptions) -> Result<Branch>` that calls `create_branch::run(&state, name, options).await`; (b) `pub mod create_branch { pub async fn run(state: &AppState, …) -> Result<…> }` containing the body (`use super::*;` inside); (c) under `#[cfg(feature = "agent")]`, `pub fn call(state: &AppState, args: serde_json::Value) -> crate::agent::CommandFuture<'_>` inside that module, deserializing a generated `#[derive(serde::Deserialize)] #[serde(rename_all = "camelCase")] struct Args { … }`. Doc comments must reach the Tauri fn so specta output is unchanged.
  - `crate::agent::CommandFuture<'a> = Pin<Box<dyn Future<Output = Result<serde_json::Value, serde_json::Value>> + Send + 'a>>`
  - `crate::agent::CommandFn = for<'a> fn(&'a AppState, serde_json::Value) -> CommandFuture<'a>`
  - `crate::agent::dispatch(state: &AppState, command: &str, args: serde_json::Value) -> Result<serde_json::Value, serde_json::Value>` (async). Unknown command → `Err(json of AxisError::Other(format!("Unknown command: {command}")))`; bad args → `Err(json of AxisError::SerializationError(e.to_string()))`; command error → `Err(serde_json::to_value(&axis_error))`.
  - `crate::agent::command_names() -> Vec<&'static str>` (sorted).

- [ ] **Step 1: Write failing tests** (temp repo via `tempfile` + `git2::Repository::init`, `AppState` with in-memory DB and `RecordingHost`, repo opened via `state.switch_active_repository`):

```rust
#[tokio::test] async fn dispatch_unknown_command_errors() {
    let err = dispatch(&state, "nope", json!({})).await.expect_err("unknown");
    assert_eq!(err, json!({ "type": "Other", "data": "Unknown command: nope" }));
}
#[tokio::test] async fn dispatch_without_repo_matches_tauri_error_shape() {
    let err = dispatch(&state_without_repo, "get_branches", json!({ "filter": {} })).await.expect_err("no repo");
    assert_eq!(err, json!({ "type": "NoRepositoryOpen" }));
}
#[tokio::test] async fn dispatch_rename_branch_accepts_camel_case_and_missing_option() {
    dispatch(&state, "create_branch", json!({ "name": "a", "options": {} })).await.expect("create");
    let v = dispatch(&state, "rename_branch", json!({ "oldName": "a", "newName": "b" })).await.expect("rename");
    assert_eq!(v["name"], "b");
}
#[tokio::test] async fn dispatch_bad_args_is_serialization_error() {
    let err = dispatch(&state, "rename_branch", json!({ "oldName": 1 })).await.expect_err("bad args");
    assert_eq!(err["type"], "SerializationError");
}
```
(Adjust `options` JSON to the real `CreateBranchOptions`/`BranchFilter` field names.)
- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml --features agent --lib agent::` — Expected: FAIL.
- [ ] **Step 3: Implement** the macro, `agent/dispatch.rs` (table built from an explicit list `agent_commands![crate::commands::create_branch, …]` that maps each path to `path::call`), and convert every command in `commands/branches.rs`.
- [ ] **Step 4: Run** the agent tests, full `cargo test`, bindings export, `git diff --exit-code src/bindings/api.ts` — Expected: PASS, no diff.
- [ ] **Step 5: Commit** `git commit -m "feat: add axis_command macro and agent dispatch for branch commands"`

---

### Task 6: Convert all remaining commands

**Files:**
- Modify: every file in `src-tauri/src/commands/` except `macros.rs`; `src-tauri/src/agent/dispatch.rs` (list)
- Test: `src-tauri/src/agent/dispatch.rs`

**Interfaces:**
- Consumes: `axis_command!`, `dispatch`, `command_names` (Task 5); `AppState::host`, `AppState::app_data_dir` (Task 3).
- Produces: `crate::agent::APP_ONLY_COMMANDS: &[&str] = &["show_in_folder", "open_terminal", "check_for_update", "download_and_install_update", "restart_app"]`; these stay plain `#[tauri::command]` functions and are not in the agent table. Every other command in `api.ts` is in the agent table. `get_file_blob` stays a plain Tauri command in this plan (Plan 2 adds its binary agent path).

- [ ] **Step 1: Write the failing coverage test**

```rust
#[test]
fn agent_table_covers_every_bound_command() {
    let api = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/bindings/api.ts")).expect("api.ts");
    let re = regex::Regex::new(r#"TAURI_INVOKE\("([a-z0-9_]+)""#).expect("regex");
    let mut bound: Vec<&str> = re.captures_iter(&api).filter_map(|c| c.get(1)).map(|m| m.as_str())
        .filter(|n| !APP_ONLY_COMMANDS.contains(n)).collect();
    bound.sort(); bound.dedup();
    assert_eq!(command_names(), bound);
}
```
- [ ] **Step 2: Run** it — Expected: FAIL listing missing commands.
- [ ] **Step 3: Convert** each module to `axis_command!`. Rules: `State<'_, AppState>` param becomes the macro's `state`; `AppHandle` params are removed in favour of `state.host()?` (open_url, start_oauth) or `state.app_data_dir()?` (custom_actions); `cancel_operation` becomes `async` (TypeScript output is already `async`, so `api.ts` does not change). Keep command order and doc comments unchanged.
- [ ] **Step 4: Run** the test, full `cargo test --features agent`, bindings export, `git diff --exit-code src/bindings/api.ts` — Expected: PASS, no diff.
- [ ] **Step 5: Commit** `git commit -m "refactor: declare all commands with axis_command"`

---

### Task 7: Make Tauri optional (`app` feature) and prove the agent build is Tauri-free

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/menu.rs` (module gated), `src-tauri/src/commands/update.rs`, `src-tauri/src/state.rs` (`pending_update`), `src-tauri/src/events/*.rs` (`#[cfg_attr(feature = "app", derive(tauri_specta::Event))]`), `src-tauri/src/host.rs` (`TauriHost` gated), `src-tauri/src/runtime.rs` (tokio branch), `src-tauri/src/commands/macros.rs` (Tauri half gated), `.github/workflows/tests.yml`, `.github/workflows/checks.yml`

**Interfaces:**
- Produces:
  - `[features] default = ["app"]`, `app = ["dep:tauri", "dep:tauri-build", "dep:tauri-specta", "dep:specta-typescript", "dep:tauri-plugin-opener", "dep:tauri-plugin-dialog", "dep:tauri-plugin-fs", "dep:tauri-plugin-shell", "dep:tauri-plugin-notifications", "dep:tauri-plugin-clipboard-manager", "dep:tauri-plugin-process"]`, `agent = []`; desktop-only `tauri-plugin-single-instance` / `tauri-plugin-updater` become optional and are enabled by `app` (target-gated as today).
  - `build.rs`: runs `tauri_build` only under `#[cfg(feature = "app")]`; always prints `cargo::rustc-check-cfg=cfg(desktop)` and `cargo::rustc-check-cfg=cfg(mobile)` so the agent build has no `unexpected_cfgs` warnings.
  - `runtime.rs`: `#[cfg(feature = "app")]` uses `tauri::async_runtime`, otherwise `tokio::spawn` / `tokio::task::spawn_blocking` and `tokio::task::JoinHandle`.
  - Updater state and commands gated `#[cfg(desktop)]`.
  - `get_specta_builder`, `run()` and the `export_typescript_bindings` test gated `#[cfg(feature = "app")]`.

- [ ] **Step 1: Write the failing check** (shell, add as CI steps too):

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml --no-default-features --features agent --lib -- -D warnings -D clippy::unwrap_used
! cargo tree --manifest-path src-tauri/Cargo.toml --no-default-features --features agent -e normal,build | grep -E '^.*\btauri(-build|-specta|-plugin-[a-z-]+)? v'
```
- [ ] **Step 2: Run** — Expected: FAIL (Tauri is still a hard dependency).
- [ ] **Step 3: Implement** the feature gates above. Add the two commands from Step 1 plus `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features agent --lib` to `tests.yml` (`test-cargo` job), and change its coverage command to `cargo llvm-cov --workspace --all-targets --features agent …` so agent tests count toward the 97% gate.
- [ ] **Step 4: Run** Step 1's commands (PASS), `cargo test --manifest-path src-tauri/Cargo.toml --all-features`, `cargo clippy --all-targets --all-features … -D warnings -D clippy::unwrap_used`, `cargo fmt --check`, bindings export + `git diff --exit-code src/bindings/api.ts`, `cargo llvm-cov --manifest-path src-tauri/Cargo.toml --workspace --all-targets --features agent --summary-only` (TOTAL line ≥ 97% for lines in changed files), and `pnpm tauri dev` smoke: open a repo, stage a file, commit, fetch, see progress toast — Expected: all pass, desktop behaves as before.
- [ ] **Step 5: Commit** `git commit -m "build: make Tauri optional behind the app feature"`
