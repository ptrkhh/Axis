mod file_watcher;
mod git_progress;
mod hook_progress;
mod integrations;
mod menu;
mod update;

pub use file_watcher::*;
pub use git_progress::*;
pub use hook_progress::*;
pub use integrations::*;
pub use menu::*;
pub use update::*;

use crate::host::AxisEvent;

macro_rules! axis_events {
    ($($ty:ty => $name:literal),* $(,)?) => {
        $(impl AxisEvent for $ty {
            const NAME: &'static str = $name;
        })*
    };
}

axis_events! {
    FilesChangedEvent => "files-changed-event",
    GitOperationProgressEvent => "git-operation-progress-event",
    HeadChangedEvent => "head-changed-event",
    HookProgressEvent => "hook-progress-event",
    IndexChangedEvent => "index-changed-event",
    IntegrationStatusChangedEvent => "integration-status-changed-event",
    MenuActionEvent => "menu-action-event",
    OAuthCallbackEvent => "o-auth-callback-event",
    RefChangedEvent => "ref-changed-event",
    RemoteFetchedEvent => "remote-fetched-event",
    RepositoryDirtyEvent => "repository-dirty-event",
    UpdateDownloadProgressEvent => "update-download-progress-event",
    WatchErrorEvent => "watch-error-event",
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_event_names_match_tauri_specta() {
        fn same<E: AxisEvent + tauri_specta::Event>() {
            assert_eq!(<E as AxisEvent>::NAME, <E as tauri_specta::Event>::NAME);
        }
        same::<FilesChangedEvent>();
        same::<GitOperationProgressEvent>();
        same::<HeadChangedEvent>();
        same::<HookProgressEvent>();
        same::<IndexChangedEvent>();
        same::<IntegrationStatusChangedEvent>();
        same::<MenuActionEvent>();
        same::<OAuthCallbackEvent>();
        same::<RefChangedEvent>();
        same::<RemoteFetchedEvent>();
        same::<RepositoryDirtyEvent>();
        same::<UpdateDownloadProgressEvent>();
        same::<WatchErrorEvent>();
    }
}
