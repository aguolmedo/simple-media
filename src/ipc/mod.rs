#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::Endpoint;
#[cfg(windows)]
pub use windows::Endpoint;

fn name() -> String {
    format!("simple-media-mpv-{}", std::process::id())
}
