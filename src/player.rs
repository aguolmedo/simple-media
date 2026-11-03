use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, watch};

use crate::ipc;

#[derive(Debug, Clone, Copy)]
pub enum PlayerEvent {
    Started,
    Position { position: f64, duration: f64 },
    Measured { duration: Option<f64>, fps: Option<f64> },
    Exited,
}

pub struct Launch {
    pub url: String,
    pub title: String,
    pub start: Option<f64>,
}

pub struct Player {
    stop: Option<oneshot::Sender<()>>,
    commands: mpsc::UnboundedSender<serde_json::Value>,
}

pub struct Track<'a> {
    pub path: &'a std::path::Path,
    pub title: String,
    pub lang: &'a str,
}

impl Player {
    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }

    pub fn add_subtitles(&self, tracks: &[Track]) {
        for (i, track) in tracks.iter().enumerate() {
            let flag = if i == 0 { "select" } else { "auto" };
            let command = json!({ "command": ["sub-add", track.path, flag, track.title, track.lang] });
            let _ = self.commands.send(command);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Deserialize)]
struct IpcMessage {
    event: Option<String>,
    name: Option<String>,
    data: Option<f64>,
    request_id: Option<u64>,
}

const DURATION_REQUEST: u64 = 1;
const FPS_REQUEST: u64 = 2;

#[derive(Default)]
struct Measuring {
    duration: Option<Option<f64>>,
    fps: Option<Option<f64>>,
}

impl Measuring {
    fn answer(&mut self, message: &IpcMessage) -> Option<PlayerEvent> {
        match message.request_id? {
            DURATION_REQUEST => self.duration = Some(message.data),
            FPS_REQUEST => self.fps = Some(message.data),
            _ => return None,
        }
        let (duration, fps) = (self.duration?, self.fps?);
        Some(PlayerEvent::Measured { duration, fps })
    }
}

fn spawn_mpv(launch: &Launch, endpoint: &ipc::Endpoint) -> std::io::Result<Child> {
    let mut command = Command::new("mpv");
    command
        .arg(&launch.url)
        .arg(format!("--force-media-title={}", launch.title))
        .arg(format!("--input-ipc-server={}", endpoint.address()))
        .arg("--force-window=immediate")
        .arg("--cache=yes")
        .arg("--ytdl=no")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(start) = launch.start {
        command.arg(format!("--start={start:.0}"));
    }
    command.spawn()
}

async fn connect(endpoint: &ipc::Endpoint) -> Option<impl AsyncRead + AsyncWrite + Send + 'static> {
    for _ in 0..100 {
        if let Ok(stream) = endpoint.open().await {
            return Some(stream);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

async fn send_commands(
    mut writer: impl AsyncWrite + Unpin,
    mut commands: mpsc::UnboundedReceiver<serde_json::Value>,
    mut loaded: watch::Receiver<bool>,
) {
    let observe = [json!({"command": ["observe_property", 1, "time-pos"]}), json!({"command": ["observe_property", 2, "duration"]})];
    for command in observe {
        let _ = writer.write_all(format!("{command}\n").as_bytes()).await;
    }
    if loaded.wait_for(|is_loaded| *is_loaded).await.is_err() {
        return;
    }
    let measure = [
        json!({"command": ["get_property", "duration"], "request_id": DURATION_REQUEST}),
        json!({"command": ["get_property", "container-fps"], "request_id": FPS_REQUEST}),
    ];
    for command in measure {
        let _ = writer.write_all(format!("{command}\n").as_bytes()).await;
    }
    while let Some(command) = commands.recv().await {
        if writer.write_all(format!("{command}\n").as_bytes()).await.is_err() {
            return;
        }
    }
}

async fn read_events(reader: impl AsyncRead + Unpin, loaded: watch::Sender<bool>, on_event: &(impl Fn(PlayerEvent) + Send)) {
    let mut lines = BufReader::new(reader).lines();
    let (mut position, mut duration, mut started) = (0.0, 0.0, false);
    let mut measuring = Measuring::default();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<IpcMessage>(&line) else { continue };
        if let Some(measured) = measuring.answer(&message) {
            on_event(measured);
        }
        if message.event.as_deref() == Some("file-loaded") {
            let _ = loaded.send(true);
        }
        if message.event.as_deref() != Some("property-change") {
            continue;
        }
        let Some(value) = message.data else { continue };
        match message.name.as_deref() {
            Some("time-pos") => position = value,
            Some("duration") => duration = value,
            _ => continue,
        }
        if !started && position > 0.0 {
            started = true;
            on_event(PlayerEvent::Started);
        }
        on_event(PlayerEvent::Position { position, duration });
    }
}

pub fn launch(launch: Launch, on_event: impl Fn(PlayerEvent) + Send + Sync + 'static) -> Player {
    let (stop_tx, stop_rx) = oneshot::channel();
    let (commands_tx, commands_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let endpoint = ipc::Endpoint::new();
        let Ok(mut child) = spawn_mpv(&launch, &endpoint) else {
            on_event(PlayerEvent::Exited);
            return;
        };
        let talk = async {
            if let Some(stream) = connect(&endpoint).await {
                let (reader, writer) = tokio::io::split(stream);
                let (loaded_tx, loaded_rx) = watch::channel(false);
                tokio::spawn(send_commands(writer, commands_rx, loaded_rx));
                read_events(reader, loaded_tx, &on_event).await;
            }
            std::future::pending::<()>().await
        };
        tokio::select! {
            _ = child.wait() => {}
            _ = stop_rx => { let _ = child.kill().await; }
            _ = talk => {}
        }
        drop(endpoint);
        on_event(PlayerEvent::Exited);
    });
    Player { stop: Some(stop_tx), commands: commands_tx }
}
