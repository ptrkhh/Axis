use crate::events::{
    FilesChangedEvent, HeadChangedEvent, IndexChangedEvent, RefChangedEvent, RepositoryDirtyEvent,
    WatchErrorEvent,
};
use crate::host::{self, HostHandle};
use crate::services::CommitCache;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// What a file watcher needs: where to emit events and which commit cache to invalidate.
#[derive(Clone)]
pub struct WatchContext {
    pub host: HostHandle,
    pub commit_cache: Arc<CommitCache>,
}

/// Per-repository file watcher that emits events based on active status.
/// Active repos get detailed events; inactive repos get a single `RepositoryDirtyEvent`.
pub struct FileWatcher {
    is_active: Arc<AtomicBool>,
    watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
    receiver_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

impl FileWatcher {
    /// Create a dummy file watcher for testing (no actual watching)
    #[cfg(feature = "integration")]
    pub fn dummy() -> Self {
        Self {
            is_active: Arc::new(AtomicBool::new(false)),
            watcher: Arc::new(Mutex::new(None)),
            receiver_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a new file watcher for a repository
    pub fn new(repo_path: PathBuf, ctx: WatchContext, is_active: bool) -> notify::Result<Self> {
        let is_active_flag = Arc::new(AtomicBool::new(is_active));

        let (tx, rx) = channel::<notify::Result<Event>>();

        // Create watcher with debouncing
        let config = Config::default().with_poll_interval(Duration::from_millis(500));
        let mut watcher = RecommendedWatcher::new(tx, config)?;

        // Watch the repository directory
        watcher.watch(&repo_path, RecursiveMode::Recursive)?;

        let watcher_arc = Arc::new(Mutex::new(Some(watcher)));

        // Spawn thread to handle events
        let handle = Self::spawn_event_handler(rx, repo_path, ctx, Arc::clone(&is_active_flag));
        let handle_arc = Arc::new(Mutex::new(Some(handle)));

        Ok(Self {
            is_active: is_active_flag,
            watcher: watcher_arc,
            receiver_handle: handle_arc,
        })
    }

    /// Set whether this repo is the active one (affects event emission mode)
    pub fn set_active(&self, active: bool) {
        self.is_active.store(active, Ordering::SeqCst);
    }

    /// Stop watching and clean up resources
    pub fn stop(&self) {
        // Drop the watcher to close the channel
        *self.watcher.lock() = None;

        // The receiver thread will exit when the channel is closed
        let handle = self.receiver_handle.lock().take();
        if let Some(handle) = handle {
            drop(handle);
        }
    }

    fn spawn_event_handler(
        rx: Receiver<notify::Result<Event>>,
        repo_path: PathBuf,
        ctx: WatchContext,
        is_active: Arc<AtomicBool>,
    ) -> thread::JoinHandle<()> {
        let git_dir = repo_path.join(".git");

        thread::spawn(move || {
            // Debouncing: collect events for a short period before emitting
            let mut pending_changes: Vec<PathBuf> = Vec::new();
            let mut last_emit = std::time::Instant::now();
            let debounce_duration = Duration::from_millis(100);

            // Track if we've already emitted a dirty event (for inactive repos)
            let mut dirty_emitted = false;

            // Track pending git events for debouncing
            let mut pending_index_changed = false;
            let mut pending_head_changed = false;
            let mut pending_refs: Vec<String> = Vec::new();

            loop {
                // Use timeout to allow periodic flushing
                match rx.recv_timeout(debounce_duration) {
                    Ok(Ok(event)) => {
                        let active = is_active.load(Ordering::SeqCst);

                        for path in event.paths {
                            // Skip paths outside this repo
                            if !path.starts_with(&repo_path) {
                                continue;
                            }

                            if active {
                                // Active repo: emit detailed events
                                if path.starts_with(&git_dir) {
                                    let relative = path.strip_prefix(&git_dir).unwrap_or(&path);
                                    let relative_str = relative.to_string_lossy();

                                    if relative_str == "index" || relative_str == "index.lock" {
                                        pending_index_changed = true;
                                    } else if relative_str == "HEAD" || relative_str == "HEAD.lock"
                                    {
                                        pending_head_changed = true;
                                    } else if relative_str.starts_with("refs/") {
                                        let ref_name = relative_str.to_string();
                                        if !pending_refs.contains(&ref_name) {
                                            pending_refs.push(ref_name);
                                        }
                                    }
                                } else {
                                    pending_changes.push(path);
                                }
                            } else {
                                // Inactive repo: emit dirty event once per batch
                                if !dirty_emitted {
                                    host::emit(
                                        ctx.host.as_ref(),
                                        &RepositoryDirtyEvent {
                                            path: repo_path.to_string_lossy().to_string(),
                                        },
                                    );
                                    dirty_emitted = true;
                                }
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        host::emit(
                            ctx.host.as_ref(),
                            &WatchErrorEvent {
                                message: e.to_string(),
                            },
                        );
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Timeout - flush pending changes if any and reset dirty flag
                        dirty_emitted = false;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // Channel closed, exit the thread
                        break;
                    }
                }

                // Emit pending events if debounce period has passed
                let has_pending = !pending_changes.is_empty()
                    || pending_index_changed
                    || pending_head_changed
                    || !pending_refs.is_empty();

                if has_pending && last_emit.elapsed() >= debounce_duration {
                    // Emit git events
                    if pending_index_changed {
                        host::emit(ctx.host.as_ref(), &IndexChangedEvent {});
                        pending_index_changed = false;
                    }
                    if pending_head_changed {
                        host::emit(ctx.host.as_ref(), &HeadChangedEvent {});
                        pending_head_changed = false;

                        // Invalidate commit cache on HEAD change
                        ctx.commit_cache.invalidate_repo(&repo_path);
                    }
                    if !pending_refs.is_empty() {
                        // Invalidate commit cache on ref changes
                        ctx.commit_cache.invalidate_repo(&repo_path);
                    }
                    for ref_name in std::mem::take(&mut pending_refs) {
                        host::emit(ctx.host.as_ref(), &RefChangedEvent { ref_name });
                    }

                    // Emit file changes
                    if !pending_changes.is_empty() {
                        let paths: Vec<String> = std::mem::take(&mut pending_changes)
                            .into_iter()
                            .filter_map(|p| {
                                p.strip_prefix(&repo_path)
                                    .ok()
                                    .map(|r| r.to_string_lossy().to_string())
                            })
                            .collect();

                        if !paths.is_empty() {
                            host::emit(ctx.host.as_ref(), &FilesChangedEvent { paths });
                        }
                    }
                    last_emit = std::time::Instant::now();
                }
            }
        })
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::RecordingHost;
    use crate::models::GraphOptions;
    use crate::services::CommitCacheEntry;
    use std::time::Instant;

    /// Polls `cond` for up to 3 s, returning whether it became true.
    fn wait_until(cond: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        cond()
    }

    fn temp_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("should create temp dir");
        git2::Repository::init(dir.path()).expect("should init repo");
        let path = dir
            .path()
            .canonicalize()
            .expect("should canonicalize repo path");
        (dir, path)
    }

    #[test]
    fn head_change_invalidates_commit_cache_and_emits() {
        let (_dir, path) = temp_repo();
        let host = RecordingHost::new();
        let commit_cache = Arc::new(CommitCache::new());
        let key = CommitCache::build_key(&path, &GraphOptions::default());
        commit_cache.set(
            key.clone(),
            CommitCacheEntry {
                commits: Vec::new(),
                max_lane: 0,
                has_more: false,
                is_prefetching: AtomicBool::new(false),
            },
        );
        assert!(commit_cache.get(&key).is_some());

        let ctx = WatchContext {
            host: host.clone(),
            commit_cache: Arc::clone(&commit_cache),
        };
        let watcher = FileWatcher::new(path.clone(), ctx, true).expect("should create watcher");

        std::fs::write(path.join(".git").join("HEAD"), "ref: refs/heads/other\n")
            .expect("should rewrite HEAD");

        assert!(
            wait_until(|| host.names().iter().any(|n| n == "head-changed-event")),
            "head-changed-event not emitted, got {:?}",
            host.names()
        );
        assert!(
            wait_until(|| commit_cache.get(&key).is_none()),
            "commit cache entry should be invalidated"
        );
        watcher.stop();
    }

    #[test]
    fn inactive_repo_emits_repository_dirty_only() {
        let (_dir, path) = temp_repo();
        let host = RecordingHost::new();
        let ctx = WatchContext {
            host: host.clone(),
            commit_cache: Arc::new(CommitCache::new()),
        };
        let watcher = FileWatcher::new(path.clone(), ctx, false).expect("should create watcher");

        std::fs::write(path.join("file.txt"), "hello").expect("should write file");

        assert!(
            wait_until(|| host.names().iter().any(|n| n == "repository-dirty-event")),
            "repository-dirty-event not emitted, got {:?}",
            host.names()
        );
        assert!(!host.names().iter().any(|n| n == "files-changed-event"));
        watcher.stop();
    }
}
