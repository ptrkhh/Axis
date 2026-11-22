//! `axis_command!` declares a command once and emits both the Tauri command and a
//! Tauri-free runner the agent can call.
//!
//! ```ignore
//! axis_command! {
//!     /// Rename a branch
//!     pub async fn rename_branch(
//!         state,
//!         old_name: String,
//!         new_name: String,
//!         force: Option<bool>,
//!     ) -> Result<Branch> {
//!         // `state` is `&AppState` here; the file's own `use` items are in scope.
//!         state.get_git_service()?.write().await.rename_branch(&old_name, &new_name, force.unwrap_or(false)).await
//!     }
//! }
//! ```
//!
//! Expands to:
//! - `#[tauri::command] #[specta::specta] pub async fn rename_branch(state: tauri::State<'_, AppState>, ..)`
//!   with the same name, params and doc comment, so bindings and `collect_commands!` are unchanged.
//! - `pub mod rename_branch { pub async fn run(state: &AppState, ..) }` holding the body.
//! - With the `agent` feature, `rename_branch::call(&AppState, serde_json::Value)`, which reads
//!   camelCase args like Tauri does (a missing `Option` arg is `None`), for `crate::agent::dispatch`.
//!
//! Attributes before the fn (doc comments, `#[allow(..)]`, `#[cfg(..)]`) apply to both the
//! Tauri fn and the module. Only `async fn` with `state` as the first param is supported.
macro_rules! axis_command {
    (
        $(#[$attr:meta])*
        $vis:vis async fn $name:ident(
            $state:ident $(, $arg:ident: $ty:ty)* $(,)?
        ) -> $ret:ty $body:block
    ) => {
        $(#[$attr])*
        #[tauri::command]
        #[specta::specta]
        $vis async fn $name(
            $state: tauri::State<'_, $crate::state::AppState>,
            $($arg: $ty),*
        ) -> $ret {
            $name::run(&$state, $($arg),*).await
        }

        $(#[$attr])*
        $vis mod $name {
            use super::*;

            pub async fn run($state: &$crate::state::AppState, $($arg: $ty),*) -> $ret $body

            #[cfg(feature = "agent")]
            pub fn call(
                state: &$crate::state::AppState,
                args: serde_json::Value,
            ) -> $crate::agent::CommandFuture<'_> {
                #[derive(serde::Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Args {
                    $($arg: $ty),*
                }

                Box::pin(async move {
                    let Args { $($arg),* } = $crate::agent::parse_args(stringify!($name), args)?;
                    $crate::agent::into_reply(run(state, $($arg),*).await)
                })
            }
        }
    };
}
