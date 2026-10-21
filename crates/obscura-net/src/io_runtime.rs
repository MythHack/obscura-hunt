use std::future::Future;
use std::sync::LazyLock;

// Every native browser transport owns its sockets on this reactor. A Window
// may be blocked in synchronous XHR without starving a previously pooled socket.
static NETWORK_IO: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("obscura-network")
        .enable_all()
        .build()
        .expect("native network runtime")
});

pub fn network_io_handle() -> &'static tokio::runtime::Handle { NETWORK_IO.handle() }

pub async fn on_network_io<F>(future: F) -> Result<F::Output, String>
where F: Future + Send + 'static, F::Output: Send + 'static {
    let task = NETWORK_IO.spawn(future);
    // Dropping an owner/cancelled request must cancel its native I/O too.
    struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
    impl<T> Drop for AbortOnDrop<T> { fn drop(&mut self) { self.0.abort(); } }
    let mut task = AbortOnDrop(task);
    (&mut task.0).await.map_err(|error| error.to_string())
}
