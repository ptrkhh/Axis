use crate::events::RemoteFetchedEvent;
use crate::host::{self, HostHandle};
use crate::models::{FetchOptions, ListRemoteOptions, SshCredentials, SshKeyFormat};
use crate::runtime::JoinHandle;
use crate::services::SshKeyService;
use crate::state::{RepositoryCache, SshPassphraseCache};
use crate::storage::Database;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

/// What the background fetch task needs from the app.
pub struct FetchContext {
    pub cache: Arc<RepositoryCache>,
    pub database: Arc<Database>,
    pub passphrases: Arc<SshPassphraseCache>,
    pub host: HostHandle,
}

/// Background service for periodically fetching all cached repositories
pub struct BackgroundFetchService {
    interval_handle: Mutex<Option<JoinHandle<()>>>,
}

impl BackgroundFetchService {
    pub const fn new() -> Self {
        Self {
            interval_handle: Mutex::new(None),
        }
    }

    /// Start the background fetch task
    /// `interval_minutes`: fetch interval in minutes (0 = disabled)
    // Allow many lines: this is a self-contained async background task that processes
    // repositories in a loop. Extracting parts would obscure the task's structure.
    #[allow(clippy::too_many_lines)]
    pub fn start(&self, ctx: FetchContext, interval_minutes: u32) {
        if interval_minutes == 0 {
            log::info!("Background fetch disabled (interval is 0)");
            return;
        }

        log::info!("Starting background fetch service with {interval_minutes} minute interval");

        let handle = crate::runtime::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(u64::from(interval_minutes) * 60));

            loop {
                interval.tick().await;

                log::debug!("Background fetch: checking cached repositories");

                // Get all cached repository paths
                let paths = ctx.cache.list_paths();

                for path in paths {
                    // Get the service handle from cache
                    if let Some(handle) = ctx.cache.get(&path) {
                        let repo_path_str = path.to_string_lossy().to_string();
                        let default_ssh_key = ctx
                            .database
                            .get_settings()
                            .await
                            .map_or(None, |s| s.default_ssh_key);

                        // List remotes (read lock)
                        let remotes = match handle
                            .read()
                            .await
                            .list_remotes(ListRemoteOptions::default())
                            .await
                        {
                            Ok(remotes) => remotes,
                            Err(e) => {
                                log::warn!(
                                    "Background fetch: failed to list remotes for {}: {e}",
                                    path.display()
                                );
                                continue;
                            }
                        };

                        let options = FetchOptions::default();
                        let mut total_updates = 0u32;

                        for remote in remotes {
                            let ssh_key = SshKeyService::resolve_ssh_key(
                                &ctx.database,
                                &repo_path_str,
                                &remote.name,
                                default_ssh_key.as_ref(),
                            )
                            .await;

                            // Skip encrypted keys when no cached passphrase is available
                            if let Some(key_path) = &ssh_key {
                                let format = SshKeyService::check_key_format_optional(
                                    std::path::Path::new(key_path),
                                );
                                if let Some(
                                    SshKeyFormat::EncryptedPem | SshKeyFormat::EncryptedOpenSsh,
                                ) = format
                                {
                                    if ctx.passphrases.get(key_path).is_none() {
                                        log::debug!(
                                            "Background fetch: skipping remote {} (encrypted key, no cached passphrase)",
                                            remote.name
                                        );
                                        continue;
                                    }
                                }
                            }

                            let ssh_creds = ssh_key.map(|key_path| {
                                let passphrase = ctx.passphrases.get(&key_path);
                                SshCredentials {
                                    key_path,
                                    passphrase,
                                }
                            });

                            // Fetch (write lock, per remote)
                            match handle
                                .write()
                                .await
                                .fetch(
                                    &remote.name,
                                    &options,
                                    None,
                                    None::<fn(&git2::Progress<'_>) -> bool>,
                                    ssh_creds,
                                )
                                .await
                            {
                                Ok(result) => {
                                    // Count updated refs as new commits
                                    total_updates += u32::try_from(result.updated_refs.len())
                                        .unwrap_or(u32::MAX);
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Background fetch failed for {} remote {}: {e}",
                                        path.display(),
                                        remote.name
                                    );
                                }
                            }
                        }

                        if total_updates > 0 {
                            log::info!(
                                "Background fetch: {total_updates} updates in {}",
                                path.display()
                            );

                            // Emit event for this repo
                            host::emit(
                                ctx.host.as_ref(),
                                &RemoteFetchedEvent {
                                    path: path.to_string_lossy().to_string(),
                                    new_commits: total_updates,
                                },
                            );
                        }
                    }
                }
            }
        });

        *self.interval_handle.lock() = Some(handle);
    }

    /// Stop the background fetch task
    pub fn stop(&self) {
        let handle = self.interval_handle.lock().take();
        if let Some(handle) = handle {
            log::info!("Stopping background fetch service");
            handle.abort();
        }
    }

    /// Restart the background fetch task with a new interval
    pub fn restart(&self, ctx: FetchContext, interval_minutes: u32) {
        self.stop();
        self.start(ctx, interval_minutes);
    }

    #[cfg(test)]
    /// Check if the background fetch service is running
    pub fn is_running(&self) -> bool {
        self.interval_handle.lock().is_some()
    }
}

impl Default for BackgroundFetchService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::RecordingHost;

    #[tokio::test]
    async fn start_and_stop_with_fetch_context() {
        let cache = Arc::new(RepositoryCache::new());
        let ctx = FetchContext {
            cache: Arc::clone(&cache),
            database: Arc::new(
                Database::open_in_memory()
                    .await
                    .expect("should create in-memory db"),
            ),
            passphrases: Arc::new(SshPassphraseCache::default()),
            host: RecordingHost::new(),
        };
        let service = BackgroundFetchService::new();

        service.start(ctx, 1);
        assert!(service.is_running());

        service.stop();
        assert!(!service.is_running());

        // The aborted task drops its context; poll until its clone of the cache is released.
        for _ in 0..300 {
            if Arc::strong_count(&cache) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(Arc::strong_count(&cache), 1, "fetch task should be dropped");
    }
}
