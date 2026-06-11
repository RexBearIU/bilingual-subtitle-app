//! Small shared helpers.

/// Poll `url` with GET until it returns 200 or `timeout_secs` expires.
/// Used to wait for sidecar servers (asr-srv, llama-server) to come up.
pub fn wait_for_http_ok(url: &str, timeout_secs: u64) -> bool {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        if ureq::get(url).call().is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    false
}
