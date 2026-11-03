use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

const APP: &str = "simple-media";

fn base_dir(dir: Option<PathBuf>) -> PathBuf {
    dir.unwrap_or_else(std::env::temp_dir).join(APP)
}

pub fn cache_dir() -> PathBuf {
    base_dir(dirs::cache_dir())
}

fn data_dir() -> PathBuf {
    base_dir(dirs::data_dir())
}

pub fn library_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir).join("movies")
}

fn data_file(name: &str) -> PathBuf {
    data_dir().join(name)
}

pub fn save_json(name: &str, value: &impl Serialize) {
    let _ = write_json(&data_file(name), value);
}

pub fn load_json<T: DeserializeOwned + Default>(name: &str) -> T {
    read_json(&data_file(name))
}

fn write_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(temporary, path)
}

fn read_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    let Ok(bytes) = std::fs::read(path) else { return T::default() };
    match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            let _ = std::fs::rename(path, path.with_extension("json.broken"));
            T::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_unreadable_files_aside() {
        let dir = std::env::temp_dir().join(format!("simple-media-test-{}", std::process::id()));
        let path = dir.join("history.json");
        write_json(&path, &vec![1, 2, 3]).unwrap();
        assert_eq!(read_json::<Vec<u32>>(&path), vec![1, 2, 3]);

        std::fs::write(&path, "[1, 2,").unwrap();
        assert!(read_json::<Vec<u32>>(&path).is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("history.json.broken")).unwrap(), "[1, 2,");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
