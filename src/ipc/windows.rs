use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

pub struct Endpoint(String);

impl Endpoint {
    pub fn new() -> Self {
        Self(format!(r"\\.\pipe\{}", super::name()))
    }

    pub fn address(&self) -> String {
        self.0.clone()
    }

    pub async fn open(&self) -> std::io::Result<NamedPipeClient> {
        ClientOptions::new().open(&self.0)
    }
}
