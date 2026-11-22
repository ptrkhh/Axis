use super::{error_value, CommandFn};
use crate::error::AxisError;
use crate::state::AppState;
use serde_json::Value;

/// Takes the same `crate::commands::<name>` paths as `collect_commands!` in `lib.rs`.
macro_rules! agent_commands {
    ($(crate::commands::$name:ident),* $(,)?) => {
        &[$((stringify!($name), crate::commands::$name::call as CommandFn)),*]
    };
}

static COMMANDS: &[(&str, CommandFn)] = agent_commands![
    // Branch commands
    crate::commands::create_branch,
    crate::commands::delete_branch,
    crate::commands::delete_remote_branch,
    crate::commands::rename_branch,
    crate::commands::checkout_branch,
    crate::commands::checkout_remote_branch,
    crate::commands::get_branch,
    crate::commands::set_branch_upstream,
    crate::commands::compare_branches,
];

/// Names of all commands the agent can run, sorted.
pub fn command_names() -> Vec<&'static str> {
    let mut names: Vec<_> = COMMANDS.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    names
}

/// Run `command` (the Tauri command name, e.g. `create_branch`) with the camelCase
/// `args` the frontend sends.
/// Errors are serialized exactly as Tauri sends them to the frontend.
pub async fn dispatch(state: &AppState, command: &str, args: Value) -> Result<Value, Value> {
    let Some((_, call)) = COMMANDS.iter().find(|(name, _)| *name == command) else {
        log::warn!("Unknown agent command: {command}");
        return Err(error_value(&AxisError::Other(format!(
            "Unknown command: {command}"
        ))));
    };
    call(state, args).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::RecordingHost;
    use crate::storage::Database;
    use serde_json::json;
    use tempfile::TempDir;

    async fn state_without_repo() -> (AppState, TempDir) {
        let db = Database::open_in_memory()
            .await
            .expect("should create in-memory db");
        let state = AppState::new(db);
        let data_dir = tempfile::tempdir().expect("should create app data dir");
        state.attach_host(RecordingHost::new(), data_dir.path().to_path_buf());
        (state, data_dir)
    }

    /// Repo with one commit and a repo-local identity (no global git config needed).
    fn init_repo() -> TempDir {
        let repo_dir = tempfile::tempdir().expect("should create repo dir");
        let repo = git2::Repository::init(repo_dir.path()).expect("should init repo");
        let mut config = repo.config().expect("should open repo config");
        config
            .set_str("user.name", "Test User")
            .expect("should set repo user.name");
        config
            .set_str("user.email", "test@example.com")
            .expect("should set repo user.email");
        let sig = repo.signature().expect("should read repo signature");
        let tree_id = repo
            .index()
            .expect("should open index")
            .write_tree()
            .expect("should write tree");
        let tree = repo.find_tree(tree_id).expect("should find tree");
        repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
            .expect("should create initial commit");
        repo_dir
    }

    async fn state_with_repo() -> (AppState, TempDir, TempDir) {
        let (state, data_dir) = state_without_repo().await;
        let repo_dir = init_repo();
        let path = repo_dir
            .path()
            .canonicalize()
            .expect("should canonicalize repo path");
        state
            .switch_active_repository(&path)
            .await
            .expect("should open repo");
        (state, data_dir, repo_dir)
    }

    #[tokio::test]
    async fn dispatch_unknown_command_errors() {
        let (state, _data) = state_without_repo().await;
        let err = dispatch(&state, "nope", json!({}))
            .await
            .expect_err("unknown command should fail");
        assert_eq!(
            err,
            json!({ "type": "Other", "data": "Unknown command: nope" })
        );
    }

    #[tokio::test]
    async fn dispatch_without_repo_matches_tauri_error_shape() {
        let (state, _data) = state_without_repo().await;
        let err = dispatch(
            &state,
            "create_branch",
            json!({ "name": "a", "options": { "force": false } }),
        )
        .await
        .expect_err("no repo open should fail");
        assert_eq!(err, json!({ "type": "NoRepositoryOpen" }));
    }

    #[tokio::test]
    async fn dispatch_rename_branch_accepts_camel_case_and_missing_option() {
        let (state, _data, _repo) = state_with_repo().await;
        let created = dispatch(
            &state,
            "create_branch",
            json!({ "name": "a", "options": { "force": false } }),
        )
        .await
        .expect("create_branch should succeed");
        assert_eq!(created["name"], "a");
        let renamed = dispatch(
            &state,
            "rename_branch",
            json!({ "oldName": "a", "newName": "b" }),
        )
        .await
        .expect("rename_branch should succeed");
        assert_eq!(renamed["name"], "b");
    }

    #[tokio::test]
    async fn dispatch_bad_args_is_serialization_error() {
        let (state, _data, _repo) = state_with_repo().await;
        let err = dispatch(&state, "rename_branch", json!({ "oldName": 1 }))
            .await
            .expect_err("bad args should fail");
        assert_eq!(err["type"], "SerializationError");
    }

    #[test]
    fn command_names_are_sorted_and_unique() {
        let names = command_names();
        assert!(names.windows(2).all(|w| w[0] < w[1]), "{names:?}");
        assert!(names.contains(&"rename_branch"));
    }
}
