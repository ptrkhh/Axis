// Async runtime seam so services do not name `tauri::async_runtime` directly.
pub use tauri::async_runtime::{spawn, spawn_blocking, JoinHandle};

#[cfg(test)]
mod tests {
    #[test]
    fn spawn_works_outside_tokio_context() {
        // Plain std thread with no runtime entered, mirroring Tauri's `.setup()`.
        let out = std::thread::spawn(|| crate::runtime::spawn(async { 7 }))
            .join()
            .expect("spawning thread should not panic");
        assert_eq!(
            tauri::async_runtime::block_on(out).expect("spawned task should complete"),
            7
        );
    }
}
