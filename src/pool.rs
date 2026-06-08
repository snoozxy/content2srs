use rayon::ThreadPoolBuilder;

pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
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
