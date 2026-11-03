use std::io::SeekFrom;

use anyhow::{Result, bail};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::http;

const HASH_CHUNK: u64 = 65536;

fn is_remote(location: &str) -> bool {
    location.starts_with("http://") || location.starts_with("https://")
}

pub async fn read(location: &str) -> Result<Vec<u8>> {
    match is_remote(location) {
        true => http::get_bytes(location).await,
        false => Ok(tokio::fs::read(location).await?),
    }
}

async fn read_file_range(path: &str, start: u64, end: u64) -> Result<Vec<u8>> {
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(SeekFrom::Start(start)).await?;
    let mut bytes = Vec::new();
    file.take(end - start + 1).read_to_end(&mut bytes).await?;
    Ok(bytes)
}

async fn read_range(url: &str, start: u64, end: u64) -> Result<Vec<u8>> {
    if !is_remote(url) {
        return read_file_range(url, start, end).await;
    }
    let res = http::get(url).header("range", format!("bytes={start}-{end}")).send().await?.error_for_status()?;
    Ok(res.bytes().await?.to_vec())
}

fn sum_words(bytes: &[u8]) -> u64 {
    bytes.as_chunks::<8>().0.iter().fold(0u64, |sum, word| sum.wrapping_add(u64::from_le_bytes(*word)))
}

pub async fn opensubtitles_hash(url: &str, size: u64) -> Result<String> {
    if size < HASH_CHUNK * 2 {
        bail!("video too small to hash");
    }
    let (head, tail) = tokio::try_join!(
        read_range(url, 0, HASH_CHUNK - 1),
        read_range(url, size - HASH_CHUNK, size - 1)
    )?;
    if head.len() as u64 != HASH_CHUNK || tail.len() as u64 != HASH_CHUNK {
        bail!("incomplete video read");
    }
    let hash = size.wrapping_add(sum_words(&head)).wrapping_add(sum_words(&tail));
    Ok(format!("{hash:016x}"))
}
