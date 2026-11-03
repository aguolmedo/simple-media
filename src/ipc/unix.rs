use std::path::PathBuf;

use tokio::net::UnixStream;

pub struct Endpoint(PathBuf);

impl Endpoint {
    pub fn new() -> Self {
        let endpoint = Self(std::env::temp_dir().join(format!("{}.sock", super::name())));
        endpoint.remove();
        endpoint
    }

    pub fn address(&self) -> String {
        self.0.display().to_string()
    }

    pub async fn open(&self) -> std::io::Result<UnixStream> {
        UnixStream::connect(&self.0).await
    }

    fn remove(&self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.remove();
    }
}
