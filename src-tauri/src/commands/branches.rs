use crate::error::Result;
use crate::models::{
    Branch, BranchCompareResult, BranchType, CheckoutOptions, CreateBranchOptions,
    DeleteBranchOptions,
};
use crate::services::HookProgressEmitter;

axis_command! {
    /// Create a new branch
    pub async fn create_branch(
        state,
        name: String,
        options: CreateBranchOptions,
    ) -> Result<Branch> {
        state
            .get_git_service()?
            .write()
            .await
            .create_branch(&name, &options)
            .await
    }
}

axis_command! {
    /// Delete a branch
    pub async fn delete_branch(
        state,
        name: String,
        options: DeleteBranchOptions,
    ) -> Result<()> {
        let ssh_creds = if options.delete_remote {
            state.resolve_ssh_credentials("origin").await?
        } else {
            None
        };
        state
            .get_git_service()?
            .write()
            .await
            .delete_branch(&name, options, ssh_creds)
            .await
    }
}

axis_command! {
    /// Rename a branch
    pub async fn rename_branch(
        state,
        old_name: String,
        new_name: String,
        force: Option<bool>,
    ) -> Result<Branch> {
        state
            .get_git_service()?
            .write()
            .await
            .rename_branch(&old_name, &new_name, force.unwrap_or(false))
            .await
    }
}

axis_command! {
    /// Checkout a branch
    pub async fn checkout_branch(
        state,
        name: String,
        options: CheckoutOptions,
    ) -> Result<()> {
        let settings = state.get_settings().await?;
        let git_service = state.get_git_service()?;
        let guard = git_service.write().await;

        // Get HEAD before checkout for post-checkout hook
        let prev_head = guard.get_head_oid().await;

        // Perform checkout
        guard.checkout_branch(&name, &options).await?;

        // Run post-checkout hook (informational, don't fail on error)
        if !settings.bypass_hooks {
            let host = state.host()?;
            let emitter = HookProgressEmitter::new(host, state.progress_registry());
            let new_head = guard.get_head_oid().await;
            let result = guard
                .run_post_checkout(&prev_head, &new_head, true, Some(&emitter))
                .await;
            if !result.skipped && !result.success {
                log::warn!("post-checkout hook failed: {}", result.stderr);
            }
        }

        drop(guard);
        Ok(())
    }
}

axis_command! {
    /// Checkout a remote branch locally
    pub async fn checkout_remote_branch(
        state,
        remote_name: String,
        branch_name: String,
        local_name: Option<String>,
        force: bool,
    ) -> Result<()> {
        let settings = state.get_settings().await?;
        let git_service = state.get_git_service()?;
        let guard = git_service.write().await;

        // Get HEAD before checkout for post-checkout hook
        let prev_head = guard.get_head_oid().await;

        // Perform checkout
        guard
            .checkout_remote_branch(&remote_name, &branch_name, local_name.as_deref(), force)
            .await?;

        // Run post-checkout hook (informational, don't fail on error)
        if !settings.bypass_hooks {
            let host = state.host()?;
            let emitter = HookProgressEmitter::new(host, state.progress_registry());
            let new_head = guard.get_head_oid().await;
            let result = guard
                .run_post_checkout(&prev_head, &new_head, true, Some(&emitter))
                .await;
            if !result.skipped && !result.success {
                log::warn!("post-checkout hook failed: {}", result.stderr);
            }
        }

        drop(guard);

        Ok(())
    }
}

axis_command! {
    /// Get branch details
    pub async fn get_branch(
        state,
        name: String,
        branch_type: BranchType,
    ) -> Result<Branch> {
        state
            .get_git_service()?
            .read()
            .await
            .get_branch(&name, branch_type)
            .await
    }
}

axis_command! {
    /// Set the upstream branch for a local branch
    pub async fn set_branch_upstream(
        state,
        branch_name: String,
        upstream: Option<String>,
    ) -> Result<()> {
        state
            .get_git_service()?
            .write()
            .await
            .set_branch_upstream(&branch_name, upstream.as_deref())
            .await
    }
}

axis_command! {
    /// Delete a remote branch
    pub async fn delete_remote_branch(
        state,
        remote_name: String,
        branch_name: String,
        force: Option<bool>,
    ) -> Result<()> {
        let ssh_creds = state.resolve_ssh_credentials(&remote_name).await?;
        state
            .get_git_service()?
            .write()
            .await
            .delete_remote_branch(
                &remote_name,
                &branch_name,
                force.unwrap_or(false),
                ssh_creds,
            )
            .await
    }
}

axis_command! {
    /// Compare two branches to find commits ahead/behind and file differences
    pub async fn compare_branches(
        state,
        base_ref: String,
        compare_ref: String,
    ) -> Result<BranchCompareResult> {
        state
            .get_git_service()?
            .read()
            .await
            .compare_branches(&base_ref, &compare_ref)
            .await
    }
}
