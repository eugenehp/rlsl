//! `lsl-rec-gui` — eGUI-based LSL recorder with live signal viewer.

use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use lsl_core::clock::local_clock;
use lsl_core::inlet::StreamInlet;
use lsl_core::resolver;
use lsl_core::signal_quality::SignalQuality;
use lsl_core::stream_info::StreamInfo;
use lsl_rec::markers::MarkerOutlet;
use lsl_rec::recording::{Recording, RecordingFormat};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;

fn main() -> eframe::Result<()> {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let _guard = rt.enter();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 720.0])
            .with_min_inner_size([640.0, 480.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "lsl-rec-gui — LSL Recorder",
        options,
        Box::new(|cc| {
            let mut style = (*cc.egui_ctx.style()).clone();
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            cc.egui_ctx.set_style(style);
            Ok(Box::new(RecorderApp::new()))
        }),
    )
}

// ── Types ────────────────────────────────────────────────────────────

struct StreamEntry {
    info: StreamInfo,
    checked: bool,
}

struct StopResult {
    filename: String,
    format: RecordingFormat,
    size: u64,
}

/// Live viewer: pulls data from a single stream for visualization.
struct LiveViewer {
    inlet: StreamInlet,
    name: String,
    nch: usize,
    ring: Vec<VecDeque<f64>>, // ring buffer per channel
    quality: SignalQuality,
    _ring_len: usize,
}

impl LiveViewer {
    fn new(info: &StreamInfo) -> Self {
        let nch = info.channel_count() as usize;
        let ring_len = 500;
        let inlet = StreamInlet::new(info, 360, 0, true);
        // open_stream is blocking — spawn it
        let inlet_ref = unsafe { &*(&inlet as *const StreamInlet) };
        let _ = inlet_ref.open_stream(5.0);
        LiveViewer {
            inlet,
            name: info.name(),
            nch,
            ring: (0..nch)
                .map(|_| VecDeque::from(vec![0.0; ring_len]))
                .collect(),
            quality: SignalQuality::new(info.nominal_srate(), nch),
            _ring_len: ring_len,
        }
    }

    /// Pull all available samples and update ring buffers.
    fn poll(&mut self) {
        let mut buf = vec![0.0f64; self.nch];
        for _ in 0..512 {
            match self.inlet.pull_sample_d(&mut buf, 0.0) {
                Ok(ts) if ts > 0.0 => {
                    self.quality.update(ts, &buf);
                    for (ch, &v) in buf.iter().enumerate() {
                        if ch < self.ring.len() {
                            self.ring[ch].pop_front();
                            self.ring[ch].push_back(v);
                        }
                    }
                }
                _ => break,
            }
        }
    }
}

// ── App state ────────────────────────────────────────────────────────

struct RecorderApp {
    streams: Vec<StreamEntry>,
    recording: Option<Recording>,
    start_time: f64,
    status_msg: String,
    format: RecordingFormat,
    resolve_rx: Option<std::sync::mpsc::Receiver<Vec<StreamInfo>>>,
    stop_rx: Option<std::sync::mpsc::Receiver<StopResult>>,
    /// Active tab
    tab: Tab,
    /// Live signal viewer (for one stream at a time)
    viewer: Option<LiveViewer>,
    /// Stream inspector: selected stream index
    inspector_idx: Option<usize>,
    /// Marker outlet
    marker_outlet: Option<MarkerOutlet>,
    marker_count: u64,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Recorder,
    Viewer,
    Inspector,
}

impl RecorderApp {
    fn new() -> Self {
        let mut app = Self {
            streams: Vec::new(),
            recording: None,
            start_time: 0.0,
            status_msg: "Click 'Refresh' to discover LSL streams".into(),
            format: RecordingFormat::Xdf,
            resolve_rx: None,
            stop_rx: None,
            tab: Tab::Recorder,
            viewer: None,
            inspector_idx: None,
            marker_outlet: None,
            marker_count: 0,
        };
        app.refresh_streams();
        app
    }

    fn refresh_streams(&mut self) {
        self.status_msg = "Resolving streams…".into();
        let (tx, rx) = std::sync::mpsc::channel();
        self.resolve_rx = Some(rx);
        tokio::task::spawn_blocking(move || {
            let found = resolver::resolve_all(1.5);
            let _ = tx.send(found);
        });
    }

    fn poll_async(&mut self) {
        if let Some(ref rx) = self.resolve_rx {
            if let Ok(found) = rx.try_recv() {
                let mut new_list = Vec::new();
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
                self.status_msg = format!("Found {} stream(s)", self.streams.len());
                self.resolve_rx = None;
            }
        }
        if let Some(ref rx) = self.stop_rx {
            if let Ok(r) = rx.try_recv() {
                self.status_msg = format!(
                    "Stopped [{}]. Saved {} ({} KB)",
                    r.format.as_str(),
                    r.filename,
                    r.size / 1024
                );
                self.stop_rx = None;
            }
        }
    }

    fn start_recording(&mut self) {
        if self.recording.is_some() {
            return;
        }
        let selected: Vec<StreamInfo> = self
            .streams
            .iter()
            .filter(|s| s.checked)
            .map(|s| s.info.clone())
            .collect();
        if selected.is_empty() {
            self.status_msg = "No streams selected".into();
            return;
        }
        let stamp = timestamp_str();
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

    fn stop_recording(&mut self) {
        if let Some(rec) = self.recording.take() {
            let filename = rec.filename.clone();
            let format = rec.format;
            let size = rec.file_size();
            self.status_msg = format!("Stopping {} …", filename);
            let (tx, rx) = std::sync::mpsc::channel();
            self.stop_rx = Some(rx);
            tokio::spawn(async move {
                rec.stop().await;
                let _ = tx.send(StopResult {
                    filename,
                    format,
                    size,
                });
            });
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

    fn push_marker(&mut self, label: &str) {
        if self.marker_outlet.is_none() {
            self.marker_outlet = Some(MarkerOutlet::new("GuiMarkers"));
        }
        if let Some(ref mut m) = self.marker_outlet {
            m.push(label);
            self.marker_count = m.count();
            self.status_msg = format!("Marker '{}' (#{})  ", label, self.marker_count);
        }
    }
}

fn timestamp_str() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}", t)
}

// ── eGUI rendering ──────────────────────────────────────────────────

impl eframe::App for RecorderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_async();
        if let Some(ref mut v) = self.viewer {
            v.poll();
        }

        let needs_repaint = self.recording.is_some()
            || self.resolve_rx.is_some()
            || self.stop_rx.is_some()
            || self.viewer.is_some();
        if needs_repaint {
            ctx.request_repaint_after(std::time::Duration::from_millis(33)); // ~30fps for viewer
        }

        // ── Top bar ──
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("🧪 lsl-rec-gui");
                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Recorder, "📹 Recorder");
                ui.selectable_value(&mut self.tab, Tab::Viewer, "📈 Live Viewer");
                ui.selectable_value(&mut self.tab, Tab::Inspector, "🔍 Inspector");
            });
        });

        // ── Status bar ──
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status_msg);
                if self.recording.is_some() {
                    ui.separator();
                    // Marker buttons
                    for i in 1..=5 {
                        if ui.small_button(format!("M{}", i)).clicked() {
                            self.push_marker(&format!("M{}", i));
                        }
                    }
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Recorder => self.ui_recorder(ui),
            Tab::Viewer => self.ui_viewer(ui),
            Tab::Inspector => self.ui_inspector(ui),
        });
    }
}

impl RecorderApp {
    fn ui_recorder(&mut self, ui: &mut egui::Ui) {
        // Toolbar
        ui.horizontal(|ui| {
            let resolving = self.resolve_rx.is_some();
            if ui
                .add_enabled(!resolving, egui::Button::new("🔄 Refresh"))
                .clicked()
            {
                self.refresh_streams();
            }
            if ui.button("✅ All").clicked() {
                for s in &mut self.streams {
                    s.checked = true;
                }
            }
            if ui.button("❌ None").clicked() {
                for s in &mut self.streams {
                    s.checked = false;
                }
            }
            ui.separator();
            let is_rec = self.recording.is_some();
            ui.add_enabled_ui(!is_rec, |ui| {
                ui.label("Format:");
                ui.selectable_value(&mut self.format, RecordingFormat::Xdf, "XDF");
                ui.selectable_value(&mut self.format, RecordingFormat::Parquet, "Parquet");
            });
            ui.separator();
            if !is_rec && self.stop_rx.is_none() {
                if ui
                    .add_enabled(
                        !self.streams.is_empty(),
                        egui::Button::new("⏺ Record").fill(egui::Color32::from_rgb(180, 40, 40)),
                    )
                    .clicked()
                {
                    self.start_recording();
                }
            } else if is_rec {
                if ui
                    .button(egui::RichText::new("⏹ Stop").color(egui::Color32::WHITE))
                    .clicked()
                {
                    self.stop_recording();
                }
            } else {
                ui.spinner();
            }
        });
        ui.separator();

        // Stream list
        if self.streams.is_empty() {
            ui.label("No streams. Click Refresh.");
        } else {
            egui::ScrollArea::vertical()
                .max_height(ui.available_height() - 60.0)
                .show(ui, |ui| {
                    egui::Grid::new("sg")
                        .num_columns(6)
                        .striped(true)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            for label in ["", "Name", "Type", "Host", "Ch", "Rate"] {
                                ui.label(egui::RichText::new(label).strong());
                            }
                            ui.end_row();
                            for entry in &mut self.streams {
                                ui.checkbox(&mut entry.checked, "");
                                ui.label(entry.info.name());
                                ui.label(entry.info.type_());
                                ui.label(entry.info.hostname());
                                ui.label(format!("{}", entry.info.channel_count()));
                                ui.label(format!("{:.0}", entry.info.nominal_srate()));
                                ui.end_row();
                            }
                        });
                });
        }

        // Recording info
        ui.separator();
        ui.horizontal(|ui| {
            if let Some(ref rec) = self.recording {
                let st = rec.state.as_ref();
                ui.label(
                    egui::RichText::new(format!("● REC [{}]", rec.format.as_str()))
                        .color(egui::Color32::RED)
                        .strong(),
                );
                ui.label(format!(
                    "{}  {} streams  {} samples  {} KB  markers: {}",
                    self.elapsed_str(),
                    st.stream_count.load(Ordering::Relaxed),
                    st.sample_count.load(Ordering::Relaxed),
                    rec.file_size() / 1024,
                    self.marker_count
                ));
            } else {
                ui.label(egui::RichText::new("○ Idle").color(egui::Color32::GRAY));
            }
        });
    }

    fn ui_viewer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Select stream to view:");
            for entry in self.streams.iter() {
                if ui.selectable_label(false, entry.info.name()).clicked() {
                    self.viewer = Some(LiveViewer::new(&entry.info));
                }
            }
            if ui.button("⏹ Close viewer").clicked() {
                self.viewer = None;
            }
        });
        ui.separator();

        if let Some(ref viewer) = self.viewer {
            // Quality stats
            let snap = viewer.quality.snapshot();
            ui.horizontal(|ui| {
                ui.label(format!("📊 {} — {}ch", viewer.name, viewer.nch));
                ui.separator();
                ui.label(format!("eff. rate: {:.1} Hz", snap.effective_srate));
                ui.label(format!("jitter: {:.3} ms", snap.jitter_sec * 1000.0));
                ui.label(format!("dropouts: {:.2}%", snap.dropout_rate * 100.0));
                ui.label(format!("samples: {}", snap.total_samples));
            });
            ui.separator();

            // Waveform plot
            let nch = viewer.nch.min(16);
            let available = ui.available_height();
            let ch_height = (available / nch as f32).max(40.0);

            egui::ScrollArea::vertical().show(ui, |ui| {
                let colors = [
                    egui::Color32::from_rgb(0, 255, 255),
                    egui::Color32::from_rgb(255, 0, 255),
                    egui::Color32::from_rgb(255, 255, 0),
                    egui::Color32::from_rgb(0, 255, 0),
                    egui::Color32::from_rgb(255, 128, 0),
                    egui::Color32::from_rgb(0, 128, 255),
                    egui::Color32::from_rgb(255, 0, 128),
                    egui::Color32::from_rgb(128, 255, 0),
                ];
                for ch in 0..nch {
                    let points: PlotPoints = viewer.ring[ch]
                        .iter()
                        .enumerate()
                        .map(|(i, &v)| [i as f64, v])
                        .collect();
                    let color = colors[ch % colors.len()];
                    Plot::new(format!("ch{}", ch))
                        .height(ch_height)
                        .show_axes([false, true])
                        .allow_drag(false)
                        .allow_zoom(false)
                        .include_y(0.0)
                        .show(ui, |plot_ui| {
                            plot_ui.line(Line::new(points).color(color).name(format!("ch{}", ch)));
                        });
                }
            });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("Select a stream above to start the live viewer.");
            });
        }
    }

    fn ui_inspector(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Select stream:");
            for (i, entry) in self.streams.iter().enumerate() {
                if ui
                    .selectable_label(self.inspector_idx == Some(i), entry.info.name())
                    .clicked()
                {
                    self.inspector_idx = Some(i);
                }
            }
        });
        ui.separator();

        if let Some(idx) = self.inspector_idx {
            if let Some(entry) = self.streams.get(idx) {
                let info = &entry.info;
                egui::Grid::new("inspect")
                    .num_columns(2)
                    .spacing([20.0, 4.0])
                    .show(ui, |ui| {
                        let rows: Vec<(&str, String)> = vec![
                            ("Name", info.name()),
                            ("Type", info.type_()),
                            ("Channels", format!("{}", info.channel_count())),
                            ("Sample Rate", format!("{} Hz", info.nominal_srate())),
                            ("Format", info.channel_format().as_str().to_string()),
                            ("Source ID", info.source_id()),
                            ("UID", info.uid()),
                            ("Hostname", info.hostname()),
                            ("Session", info.session_id()),
                            ("Created", format!("{:.6}s", info.created_at())),
                            (
                                "IPv4 Data",
                                format!("{}:{}", info.v4address(), info.v4data_port()),
                            ),
                            (
                                "IPv4 Service",
                                format!("{}:{}", info.v4address(), info.v4service_port()),
                            ),
                            (
                                "IPv6 Data",
                                format!("{}:{}", info.v6address(), info.v6data_port()),
                            ),
                            (
                                "IPv6 Service",
                                format!("{}:{}", info.v6address(), info.v6service_port()),
                            ),
                        ];
                        for (k, v) in &rows {
                            ui.label(egui::RichText::new(*k).strong());
                            ui.label(v);
                            ui.end_row();
                        }
                    });

                ui.separator();
                ui.label(egui::RichText::new("XML Description").strong());
                let xml = info.to_fullinfo_message();
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        ui.code(xml);
                    });
            }
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("Select a stream to inspect.");
            });
        }
    }
}
