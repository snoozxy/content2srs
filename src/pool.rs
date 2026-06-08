use rayon::ThreadPoolBuilder;

pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
}

pub fn default_episode_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1).min(4))
        .unwrap_or(2)
}

pub fn resolve_episode_jobs(configured: Option<usize>) -> usize {
    configured.unwrap_or_else(default_episode_jobs).max(1)
}

pub fn workers_per_episode(total_workers: usize, episode_jobs: usize) -> usize {
    (total_workers / episode_jobs.max(1)).max(1)
}

pub fn install_pool_with<F, R>(workers: usize, f: F) -> R
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    let pool = ThreadPoolBuilder::new()
        .num_threads(workers.max(1))
        .build()
        .expect("failed to build rayon thread pool");
    pool.install(f)
}
