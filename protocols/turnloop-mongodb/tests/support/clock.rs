pub fn now() -> turnloop_mongodb::Instant {
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        turnloop_mongodb::Instant::now()
    }
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        turnloop_mongodb::Instant::from_duration(std::time::Duration::from_secs(1_000_000))
    }
}
