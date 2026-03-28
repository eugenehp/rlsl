//! `rlsl-rec` — TUI-based LSL recorder (async/tokio).
//!
//! Discovers LSL streams on the network and records selected ones to XDF or Parquet.
//!
//! Usage:  `cargo run -p rlsl-rec`

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::prelude::*;
use ratatui::widgets::*;
use rlsl::clock::local_clock;
use rlsl::resolver;
use rlsl::stream_info::StreamInfo;
use rlsl_rec::markers::MarkerOutlet;
use rlsl_rec::recording::{Recording, RecordingFormat};
use std::io::stdout;
use std::time::Duration;
use tokio::sync::oneshot;

// ── App state ────────────────────────────────────────────────────────

struct App {
    streams: Vec<StreamEntry>,
    selected: usize,
    recording: Option<Recording>,
    start_time: f64,
    status_msg: String,
    should_quit: bool,
    format: RecordingFormat,
    /// Pending async resolve result
    resolve_rx: Option<oneshot::Receiver<Vec<StreamInfo>>>,
    /// Marker outlet for injecting annotations
    marker_outlet: Option<MarkerOutlet>,
    marker_count: u64,
}

struct StreamEntry {
    info: StreamInfo,
    checked: bool,
}

impl App {
    fn new() -> Self {
        App {
            streams: Vec::new(),
            selected: 0,
            recording: None,
            start_time: 0.0,
            status_msg: "Press 'r' to refresh streams".into(),
            should_quit: false,
            format: RecordingFormat::Xdf,
            resolve_rx: None,
            marker_outlet: None,
            marker_count: 0,
        }
    }

    /// Kick off an async stream resolve (non-blocking).
    fn refresh_streams(&mut self) {
        self.status_msg = "Resolving streams…".into();
        let (tx, rx) = oneshot::channel();
        self.resolve_rx = Some(rx);
        tokio::task::spawn_blocking(move || {
            let found = resolver::resolve_all(1.5);
            let _ = tx.send(found);
        });
    }

    /// Poll for completed resolve. Call every tick.
    fn poll_resolve(&mut self) {
        if let Some(ref mut rx) = self.resolve_rx {
            match rx.try_recv() {
                Ok(found) => {
                    let mut new_list: Vec<StreamEntry> = Vec::new();
                    for info in found {
                        let uid = info.uid();
                        let was_checked = self
                            .streams
                            .iter()
                            .find(|s| s.info.uid() == uid)
                            .map(|s| s.checked)
                            .unwrap_or(false);
                        new_list.push(StreamEntry {
                            info,
                            checked: was_checked,
                        });
                    }
                    self.streams = new_list;
                    self.status_msg = format!(
                        "Found {} stream(s). Space=toggle, Enter=record, f=format({})",
                        self.streams.len(),
                        self.format.as_str()
                    );
                    self.resolve_rx = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => { /* still resolving */ }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.status_msg = "Resolve failed".into();
                    self.resolve_rx = None;
                }
            }
        }
    }

    fn toggle_format(&mut self) {
        self.format = match self.format {
            RecordingFormat::Xdf => RecordingFormat::Parquet,
            RecordingFormat::Parquet => RecordingFormat::Xdf,
        };
        self.status_msg = format!("Format: {} — press Enter to record", self.format.as_str());
    }

    fn toggle_selected(&mut self) {
        if let Some(s) = self.streams.get_mut(self.selected) {
            s.checked = !s.checked;
        }
    }

    fn start_recording(&mut self) {
        if self.recording.is_some() {
            self.status_msg = "Already recording!".into();
            return;
        }
        let selected: Vec<StreamInfo> = self
            .streams
            .iter()
            .filter(|s| s.checked)
            .map(|s| s.info.clone())
            .collect();
        if selected.is_empty() {
            self.status_msg = "No streams selected — toggle with Space first".into();
            return;
        }
        let stamp = chrono_ish_stamp();
        let filename = match self.format {
            RecordingFormat::Xdf => format!("recording_{}.xdf", stamp),
            RecordingFormat::Parquet => format!("recording_{}_parquet", stamp),
        };
        match Recording::start_with_format(&filename, &selected, self.format) {
            Ok(rec) => {
                self.start_time = local_clock();
                self.status_msg = format!("Recording [{}] to {} …", self.format.as_str(), filename);
                self.recording = Some(rec);
            }
            Err(e) => {
                self.status_msg = format!("Error: {}", e);
            }
        }
    }

    /// Stop recording asynchronously.
    async fn stop_recording(&mut self) {
        if let Some(rec) = self.recording.take() {
            let fname = rec.filename.clone();
            let fmt = rec.format;
            let size = rec.file_size();
            rec.stop().await;
            self.status_msg = format!(
                "Stopped [{}]. Saved {} ({} KB)",
                fmt.as_str(),
                fname,
                size / 1024
            );
        }
    }

    fn elapsed_str(&self) -> String {
        if self.recording.is_some() {
            let secs = (local_clock() - self.start_time).max(0.0) as u64;
            format!(
                "{:02}:{:02}:{:02}",
                secs / 3600,
                (secs / 60) % 60,
                secs % 60
            )
        } else {
            "—".into()
        }
    }
}

fn chrono_ish_stamp() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}", t)
}

// ── Async TUI main loop ─────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut app = App::new();
    app.refresh_streams();

    let tick_rate = Duration::from_millis(200);
    let mut event_stream = EventStream::new();

    loop {
        terminal.draw(|f| ui(f, &app))?;

        // Poll for resolved streams each tick
        app.poll_resolve();

        tokio::select! {
            _ = tokio::time::sleep(tick_rate) => {
                // periodic tick — just redraw
            }
            event = event_stream.next() => {
                if let Some(Ok(Event::Key(key))) = event {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.should_quit = true;
                        }
                        KeyCode::Char('r') if app.resolve_rx.is_none() => {
                            app.refresh_streams();
                        }
                        KeyCode::Char('f') if app.recording.is_none() => app.toggle_format(),
                        KeyCode::Up | KeyCode::Char('k') => {
                            if app.selected > 0 { app.selected -= 1; }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if app.selected + 1 < app.streams.len() { app.selected += 1; }
                        }
                        KeyCode::Char(' ') => app.toggle_selected(),
                        KeyCode::Char('a') => {
                            for s in &mut app.streams { s.checked = true; }
                        }
                        KeyCode::Char('n') => {
                            for s in &mut app.streams { s.checked = false; }
                        }
                        // Keys 1-9 inject markers during recording
                    KeyCode::Char(c @ '1'..='9') if app.recording.is_some() => {
                        if app.marker_outlet.is_none() {
                            app.marker_outlet = Some(MarkerOutlet::new("RecMarkers"));
                        }
                        if let Some(ref mut m) = app.marker_outlet {
                            let label = format!("M{}", c);
                            m.push(&label);
                            app.marker_count = m.count();
                            app.status_msg = format!("Marker '{}' injected (#{})" , label, app.marker_count);
                        }
                    }
                    KeyCode::Enter => {
                            if app.recording.is_some() {
                                app.stop_recording().await;
                            } else {
                                app.start_recording();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if app.should_quit {
            if app.recording.is_some() {
                app.stop_recording().await;
            }
            break;
        }
    }

    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    Ok(())
}

// ── UI rendering ─────────────────────────────────────────────────────

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Length(3),
        ])
        .split(f.area());

    // ── title ──
    let fmt_label = if let Some(ref rec) = app.recording {
        rec.format.as_str()
    } else {
        app.format.as_str()
    };
    let title = Paragraph::new(format!(" rlsl-rec — LSL Recorder  [format: {}]", fmt_label))
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // ── stream list ──
    let items: Vec<ListItem> = app
        .streams
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let check = if s.checked { "[x]" } else { "[ ]" };
            let label = format!(
                "{} {} ({}) — {}ch {}Hz {}",
                check,
                s.info.name(),
                s.info.hostname(),
                s.info.channel_count(),
                s.info.nominal_srate(),
                s.info.channel_format().as_str(),
            );
            let style = if i == app.selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if s.checked {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            ListItem::new(label).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Streams (↑↓ move, Space toggle, a=all, n=none, r=refresh, f=format) ")
            .borders(Borders::ALL),
    );
    f.render_widget(list, chunks[1]);

    // ── recording info ──
    let rec_text = if let Some(ref rec) = app.recording {
        let state = rec.state.as_ref();
        let samples = state
            .sample_count
            .load(std::sync::atomic::Ordering::Relaxed);
        let streams = state
            .stream_count
            .load(std::sync::atomic::Ordering::Relaxed);
        let size_kb = rec.file_size() / 1024;
        format!(
            " ● RECORDING [{}]  {}  |  {} streams  |  {} samples  |  {} KB",
            rec.format.as_str(),
            app.elapsed_str(),
            streams,
            samples,
            size_kb,
        )
    } else {
        " ○ Idle — press Enter to start recording".into()
    };
    let rec_style = if app.recording.is_some() {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let rec_widget = Paragraph::new(rec_text)
        .style(rec_style)
        .block(Block::default().title(" Recording ").borders(Borders::ALL));
    f.render_widget(rec_widget, chunks[2]);

    // ── status bar ──
    let status = Paragraph::new(format!(
        " {}  |  q=quit  Enter=start/stop  f=format",
        app.status_msg
    ))
    .style(Style::default().fg(Color::White))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(status, chunks[3]);
}
