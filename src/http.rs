use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{Client, RequestBuilder};
use serde::de::DeserializeOwned;

const TIMEOUT: Duration = Duration::from_secs(15);
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
const BROWSER_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

static CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(TIMEOUT)
        .user_agent(BROWSER_UA)
        .build()
        .expect("HTTP client")
});

static DOWNLOADS: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .connect_timeout(TIMEOUT)
        .read_timeout(STALL_TIMEOUT)
        .user_agent(BROWSER_UA)
        .build()
        .expect("HTTP client")
});

pub fn download(url: &str) -> RequestBuilder {
    DOWNLOADS.get(url)
}

pub fn get(url: &str) -> RequestBuilder {
    CLIENT.get(url)
}

pub fn post(url: &str) -> RequestBuilder {
    CLIENT.post(url)
}

async fn send(request: RequestBuilder) -> Result<reqwest::Response> {
    let res = request.send().await?;
    let status = res.status();
    if !status.is_success() {
        let host = res.url().host_str().unwrap_or("?").to_string();
        bail!("HTTP {} from {host}", status.as_u16());
    }
    Ok(res)
}

pub async fn json<T: DeserializeOwned>(request: RequestBuilder) -> Result<T> {
    let res = send(request).await?;
    let host = res.url().host_str().unwrap_or("?").to_string();
    res.json().await.with_context(|| format!("unexpected response from {host}"))
}

pub async fn get_bytes(url: &str) -> Result<Vec<u8>> {
    Ok(send(get(url)).await?.bytes().await?.to_vec())
}

pub async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    json(get(url)).await
}

pub async fn first_ok<T, F, Fut>(mirrors: &[&str], fetch: F) -> Result<T>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut last_error = None;
    for base in mirrors {
        match fetch(base.to_string()).await {
            Ok(value) => return Ok(value),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no mirrors")))
}
