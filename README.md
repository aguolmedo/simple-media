# simple-media

A terminal media center: search for movies and series, pick a torrent and watch it in
[mpv](https://mpv.io) with the best matching subtitle. Everything runs locally, with no
central server.

## Features

- **Live search** of movies and series through Cinemeta, with a details panel and poster
  rendered in the terminal.
- **Home tabs:** Continue watching, Watchlist, Downloads and Recently watched.
- **Series view:** seasons and episodes, with watched marks and progress.
- **Streams** from YTS, EZTV, The Pirate Bay, Knaben, torrents-csv and Internet Archive,
  showing quality, 👤 seeds / leechers, size, source and languages (`ENG`, `SPA`, `LAT`…).
  Season packs are resolved to the right episode file. Quick filter with `f` (e.g. `1080 spa`).
- **Built-in torrent engine** ([librqbit](https://github.com/ikatson/rqbit)) that streams to
  mpv over a local HTTP server, prioritising the pieces the player asks for.
- **Subtitles** from OpenSubtitles and YIFY, scored against the video (file hash, duration,
  fps, release group). The best one is loaded by default and the rest are added as extra
  tracks (switch with `j` in mpv).
- **Progress tracking:** playback resumes where you left off, and when an episode ends the
  next one is offered (from the same pack when possible).
- **Downloads** to `~/movies/<Title> (<Year>)/`. They keep going while you browse, resume on
  start-up, and files already in `~/movies` show up first as "On disk".

## Requirements

- [Rust](https://rustup.rs) (edition 2024)
- [mpv](https://mpv.io) in your `PATH`
- A terminal with true colour and mouse support (on Windows, Windows Terminal)

Works on Linux, macOS and Windows.

## Running

```sh
cargo run --release
```

## Keys

| Screen    | Key            | Action                          |
|-----------|----------------|---------------------------------|
| Anywhere  | `↑↓` `PgUp/PgDn` `Home/End` | Move                |
|           | `ctrl+s`       | Add to / remove from watchlist  |
|           | `ctrl+c`       | Quit                            |
| Home      | type           | Search                          |
|           | `tab` / `←→`   | Switch tab                      |
|           | `⏎`            | Open                            |
|           | `esc`          | Clear search / quit             |
| Series    | `←→`           | Switch season                   |
|           | `⏎`            | Show streams for the episode    |
| Streams   | `⏎`            | Watch                           |
|           | `s`            | Start over (when resuming)      |
|           | `d`            | Download                        |
|           | `f` or `/`     | Filter                          |
|           | `←→`           | Switch source                   |
| Playing   | `⏎`            | Watch next episode (when offered) |
|           | `esc`          | Stop and go back                |

Mouse: click to select, click again to open, scroll wheel, click on tabs and seasons.
Right click opens a menu on the home screen and goes back everywhere else.

## Files

| What                                   | Linux                              | Windows                          |
|----------------------------------------|------------------------------------|----------------------------------|
| Progress, watchlist and downloads list | `~/.local/share/simple-media/`     | `%APPDATA%\simple-media\`        |
| Posters, library cache, temporary torrent data | `~/.cache/simple-media/`   | `%LOCALAPPDATA%\simple-media\`   |
| Downloads and local library            | `~/movies/`                        | `%USERPROFILE%\movies\`          |

On macOS, data and cache live in `~/Library/Application Support/simple-media/` and
`~/Library/Caches/simple-media/`. Temporary torrent data is cleared when playback stops.

## Project layout

```
src/
├── app.rs          messages and screen stack
├── screens/        one file per screen (home, series, streams, playing)
├── streams/        torrent sources and season-pack resolution
├── subtitles/      subtitle providers and scoring
├── engine.rs       librqbit torrent engine and local HTTP server
├── player.rs       mpv launch and IPC
├── ipc/            connection to mpv (Unix socket or Windows named pipe)
├── downloads.rs    background downloads
├── library.rs      local ~/movies library
└── …               catalog, history, watchlist, poster, release parsing, storage
```


