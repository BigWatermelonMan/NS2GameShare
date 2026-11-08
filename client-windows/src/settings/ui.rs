//! GPU-rendered, resizable settings UI. The playback process is unchanged.
use super::i18n::Language;
use super::{
    native::{self, Bridge, Session},
    *,
};
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use std::{
    process::Command,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const BUTTONS: [&str; 16] = [
    "A",
    "B",
    "X",
    "Y",
    "L3",
    "R3",
    "L",
    "R",
    "ZL",
    "ZR",
    "＋",
    "－",
    "十字 左",
    "十字 上",
    "十字 右",
    "十字 下",
];
const AXES: [&str; 8] = ["X", "Y", "Z", "Rx", "Ry", "Rz", "Slider 1", "Slider 2"];
const PAGES: [(&str, &str); 6] = [
    ("接続", "Switch 2 の表示名と接続方法を設定します。"),
    ("映像と音声", "画質・音声と、設定画面の表示を調整します。"),
    (
        "キーボード",
        "ボタンをクリックして、使いたいキーを押してください。",
    ),
    ("コントローラー", "DirectInputとXInputの入力を設定します。"),
    (
        "マウス",
        "マウスからのスティック・ジャイロ操作とボタンを設定します。",
    ),
    (
        "入力テスト",
        "Switch 2 に接続せず、現在の設定で入力を確認できます。",
    ),
];
#[derive(Clone, Copy)]
struct Palette {
    background: Color32,
    card: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    tint: Color32,
    input: Color32,
}
impl Palette {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                background: Color32::from_rgb(19, 22, 29),
                card: Color32::from_rgb(27, 31, 41),
                border: Color32::from_rgb(49, 55, 70),
                text: Color32::from_rgb(236, 240, 248),
                muted: Color32::from_rgb(158, 168, 188),
                accent: Color32::from_rgb(139, 125, 247),
                tint: Color32::from_rgb(49, 43, 79),
                input: Color32::from_rgb(22, 26, 35),
            }
        } else {
            Self {
                background: Color32::from_rgb(245, 247, 251),
                card: Color32::WHITE,
                border: Color32::from_rgb(225, 230, 240),
                text: Color32::from_rgb(35, 43, 61),
                muted: Color32::from_rgb(110, 122, 144),
                accent: Color32::from_rgb(104, 86, 219),
                tint: Color32::from_rgb(238, 235, 253),
                input: Color32::from_rgb(248, 250, 253),
            }
        }
    }
}
fn theme(ctx: &egui::Context, dark: bool) {
    let p = Palette::new(dark);
    let mut style = (*ctx.style()).clone();
    style.visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    style.visuals.override_text_color = Some(p.text);
    style.visuals.panel_fill = p.background;
    style.visuals.window_fill = p.card;
    style.visuals.extreme_bg_color = p.input;
    style.visuals.faint_bg_color = p.input;
    style.visuals.selection.bg_fill = p.accent;
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.inactive.bg_fill = p.input;
    style.visuals.widgets.inactive.weak_bg_fill = p.input;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    style.visuals.widgets.hovered.bg_fill = p.tint;
    style.visuals.widgets.hovered.weak_bg_fill = p.tint;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    style.visuals.widgets.active.bg_fill = p.tint;
    style.visuals.widgets.active.weak_bg_fill = p.tint;
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
    ] {
        widget.corner_radius = egui::CornerRadius::same(7);
    }
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.spacing.interact_size = egui::vec2(40.0, 34.0);
    style.spacing.slider_width = 180.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(26.0));
    ctx.set_style(style);
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if dark {
        egui::SystemTheme::Dark
    } else {
        egui::SystemTheme::Light
    }));
}
fn fonts(ctx: &egui::Context) {
    let mut definitions = egui::FontDefinitions::default();
    let folder = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/Windows"))
        .join("Fonts");
    for (name, candidates, first) in [
        ("system-latin", vec!["segoeui.ttf"], true),
        (
            "system-japanese",
            vec!["meiryo.ttc", "YuGothM.ttc", "msgothic.ttc"],
            true,
        ),
    ] {
        for candidate in candidates {
            if let Ok(bytes) = std::fs::read(folder.join(candidate)) {
                definitions
                    .font_data
                    .insert(name.into(), egui::FontData::from_owned(bytes).into());
                let family = definitions
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default();
                if first {
                    family.insert(0, name.into());
                } else {
                    family.push(name.into());
                }
                definitions
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .insert(0, name.into());
                break;
            }
        }
    }
    ctx.set_fonts(definitions);
}
fn app_icon() -> egui::IconData {
    let mut rgba = vec![0; 64 * 64 * 4];
    let segments = [
        ([20.0_f32, 45.0_f32], [20.0, 19.0]),
        ([20.0, 19.0], [44.0, 45.0]),
        ([44.0, 45.0], [44.0, 19.0]),
    ];
    for y in 0..64 {
        for x in 0..64 {
            let point = [x as f32 + 0.5, y as f32 + 0.5];
            let dx = (point[0] - 32.0).abs() - 18.0;
            let dy = (point[1] - 32.0).abs() - 18.0;
            if dx.max(0.0).hypot(dy.max(0.0)) > 13.0 {
                continue;
            }
            let on = segments.iter().any(|(a, b)| {
                let v = [b[0] - a[0], b[1] - a[1]];
                let t = (((point[0] - a[0]) * v[0] + (point[1] - a[1]) * v[1])
                    / (v[0] * v[0] + v[1] * v[1]))
                    .clamp(0.0, 1.0);
                (point[0] - a[0] - v[0] * t).hypot(point[1] - a[1] - v[1] * t) < 2.2
            });
            let color = if on {
                [255, 255, 255, 255]
            } else {
                [126, 109, 238, 255]
            };
            rgba[(y * 64 + x) * 4..(y * 64 + x + 1) * 4].copy_from_slice(&color);
        }
    }
    egui::IconData {
        rgba,
        width: 64,
        height: 64,
    }
}
fn card(
    ui: &mut egui::Ui,
    p: Palette,
    title: &str,
    subtitle: &str,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(12)
        .inner_margin(22)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(17.0).strong());
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).size(12.5).color(p.muted));
            }
            ui.add_space(10.0);
            body(ui);
        });
    ui.add_space(14.0);
}
fn row(ui: &mut egui::Ui, label: &str, body: impl FnOnce(&mut egui::Ui)) {
    if ui.available_width() < 500.0 {
        ui.label(label);
        body(ui);
        ui.add_space(6.0);
    } else {
        ui.horizontal(|ui| {
            ui.add_sized([146.0, 34.0], egui::Label::new(label));
            body(ui);
        });
    }
}
fn key_name(key: Option<u16>, language: Language) -> String {
    let Some(k) = key else {
        return language.text("未割当").into();
    };
    match k {
        37 => "←".into(),
        38 => "↑".into(),
        39 => "→".into(),
        40 => "↓".into(),
        13 => "Enter".into(),
        8 => "Backspace".into(),
        32 => "Space".into(),
        9 => "Tab".into(),
        16 => "Shift".into(),
        17 => "Ctrl".into(),
        18 => "Alt".into(),
        112..=135 => format!("F{}", k - 111),
        48..=90 => char::from_u32(k as u32).unwrap_or('?').to_string(),
        _ => format!("VK {k:02X}"),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Binding {
    Button(usize),
    Stick(usize),
    Mouse,
    GyroReset,
}
fn assign(settings: &mut Settings, target: Binding, key: u16) -> Result<bool> {
    if key == 27 {
        return Ok(false);
    }
    let key = (key != 46).then_some(key);
    ensure!(
        key.is_none_or(crate::input::valid_key),
        "Esc と F9 は予約されています。"
    );
    match target {
        Binding::Button(i) => settings.input.keyboard.buttons[i] = key,
        Binding::Stick(i) => settings.input.keyboard.left_stick[i] = key,
        Binding::Mouse => {
            settings.input.mouse_capture_key =
                key.context("取り込みキーを未割当にすることはできません。")?
        }
        Binding::GyroReset => {
            settings.input.mouse_gyro.reset_key =
                key.context("Gyro reset key cannot be unassigned")?;
        }
    }
    Ok(true)
}
struct ExtractionJob {
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    result: std::sync::mpsc::Receiver<std::result::Result<PathBuf, String>>,
}
impl Drop for ExtractionJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
struct ControllerCapture {
    xinput: bool,
    target: usize,
    tracker: crate::binding::Tracker,
}
struct App {
    controller_capture: Option<ControllerCapture>,
    controller_sampler: Option<crate::input::Sampler>,
    extraction_open: bool,
    extraction_source: PathBuf,
    extraction: Option<ExtractionJob>,
    settings: Settings,
    path: PathBuf,
    saved: Vec<u8>,
    page: usize,
    palette: Palette,
    applied_theme: Option<bool>,
    bridge: Option<Bridge>,
    session: Option<Session>,
    last_log: Option<PathBuf>,
    last_poll: Instant,
    ports: Vec<String>,
    controllers: Vec<crate::dinput::DeviceInfo>,
    capture: Option<Binding>,
    sampler: Option<crate::input::Sampler>,
    test_sample: crate::input::State,
    test_events: u64,
    flashes: [Option<Instant>; 16],
    map_source: usize,
    status: String,
    error: Option<String>,
    closing: bool,
    close_prompt: bool,
    allow_close: bool,
}
impl App {
    fn start_extraction(&mut self) -> Result<()> {
        ensure!(
            self.extraction.is_none() && self.session.is_none(),
            "Extraction is unavailable during playback or another extraction"
        );
        ensure!(
            self.extraction_source.exists(),
            "Select a mounted SYSTEM directory or splay input file"
        );
        self.stop_test();
        self.cancel_capture();
        let source = self.extraction_source.clone();
        let keys = self.settings.keys.clone();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let output = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .join("private")
            .join(format!("gameshare-passphrase-{stamp}.bin"));
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let (sender, result) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("passphrase-extraction".into())
            .spawn(move || {
                let result =
                    crate::passphrase::extract_to_file(&source, &keys, &output, &thread_cancel)
                        .map(|_| output)
                        .map_err(|error| format!("{error:#}"));
                let _ = sender.send(result);
            })?;
        self.extraction = Some(ExtractionJob { cancel, result });
        self.status = "抽出中です。SYSTEMには書き込みません。".into();
        self.error = None;
        Ok(())
    }
    fn extraction_dialog(&mut self, ctx: &egui::Context) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        let mut opened = true;
        egui::Window::new(tr("パスフレーズの抽出"))
            .open(&mut opened).collapsible(false).default_width(600.0)
            .show(ctx, |ui| {
                ui.label(tr("所有する本体の読み取り専用SYSTEM、splay Program NCA、またはExeFSのmain NSOを選択します。"));
                ui.label(tr("NCAの復号には、接続画面で選んだprod.keysのheader_keyとkey_area_keyが必要です。"));
                ui.add_space(8.0);
                let mut source = self.extraction_source.to_string_lossy().into_owned();
                ui.add_enabled_ui(self.extraction.is_none(), |ui| {
                    if ui.add(egui::TextEdit::singleline(&mut source).desired_width(f32::INFINITY)).changed() {
                        self.extraction_source = PathBuf::from(&source);
                    }
                    ui.horizontal(|ui| {
                        if ui.button(tr("SYSTEMフォルダー…")).clicked()
                            && let Some(bridge) = &self.bridge {
                                let result = bridge.browse_folder(&mut self.extraction_source).map(|_| ());
                                self.record(result);
                            }
                        if ui.button(tr("NCA / NSOファイル…")).clicked()
                            && let Some(bridge) = &self.bridge {
                                let result = bridge.browse(&mut self.extraction_source, language).map(|_| ());
                                self.record(result);
                            }
                    });
                });
                ui.add_space(10.0);
                ui.label(tr("対応ビルドだけを検証して抽出します。未知の版では出力しません。"));
                ui.label(tr("抽出値は非表示で、設定フォルダー内のprivateへ16バイトだけ保存します。元のファイルと既存の出力は上書きしません。"));
                if let Some(error) = &self.error { ui.colored_label(Color32::from_rgb(209,75,101), language.message(error)); }
                ui.horizontal(|ui| {
                    if self.extraction.is_some() {
                        ui.spinner();
                        ui.label(tr("抽出中…"));
                        if ui.button(tr("キャンセル")).clicked()
                            && let Some(job) = &self.extraction { job.cancel.store(true, Ordering::Relaxed); }
                    } else if ui.button(tr("抽出して設定に反映")).clicked() {
                        let result = self.start_extraction(); self.record(result);
                    }
                });
            });
        self.extraction_open = opened;
        if !opened && let Some(job) = &self.extraction {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
    fn new(settings: Settings, path: PathBuf, bridge: Option<Bridge>) -> Self {
        let palette = Palette::new(settings.dark_theme);
        let saved = serde_json::to_vec(&settings).unwrap_or_default();
        Self {
            controller_capture: None,
            controller_sampler: None,
            extraction_open: false,
            extraction_source: PathBuf::new(),
            extraction: None,
            settings,
            path,
            saved,
            page: 0,
            palette,
            applied_theme: None,
            bridge,
            session: None,
            last_log: None,
            last_poll: Instant::now(),
            ports: vec![],
            controllers: vec![],
            capture: None,
            sampler: None,
            test_sample: Default::default(),
            test_events: 0,
            flashes: [None; 16],
            map_source: 0,
            status: "設定を確認して、Switch 2 の募集に接続できます。".into(),
            error: None,
            closing: false,
            close_prompt: false,
            allow_close: false,
        }
    }
    fn dirty(&self) -> bool {
        serde_json::to_vec(&self.settings).is_ok_and(|s| s != self.saved)
    }
    fn record(&mut self, result: Result<()>) {
        if let Err(e) = result {
            self.error = Some(format!("{e:#}"));
        }
    }
    fn refresh(&mut self) -> Result<()> {
        self.ports=serialport::available_ports()?.into_iter().filter(|p|matches!(&p.port_type,serialport::SerialPortType::UsbPort(u) if u.vid==0x303a&&u.pid==0x1001)).map(|p|p.port_name).collect();
        self.controllers = native::controllers()?;
        Ok(())
    }
    fn save_settings(&mut self) -> Result<()> {
        self.settings.save(&self.path)?;
        self.saved = serde_json::to_vec(&self.settings)?;
        self.status = "設定を保存しました。次の接続から適用されます。".into();
        self.error = None;
        Ok(())
    }
    fn cancel_capture(&mut self) {
        self.controller_capture = None;
        let controller_active = self.controller_sampler.take().is_some();
        self.capture = None;
        if let Some(bridge) = &self.bridge {
            bridge.cancel_capture();
            if controller_active {
                bridge.stop_test();
            }
        }
    }
    fn stop_test(&mut self) {
        if let Some(bridge) = &self.bridge {
            bridge.stop_test();
        }
        self.sampler = None;
        self.test_sample = Default::default();
    }
    fn start_capture(&mut self, target: Binding) {
        self.stop_test();
        self.cancel_capture();
        self.capture = Some(target);
        if let Some(bridge) = &self.bridge {
            bridge.begin_capture();
        }
    }
    fn start_test(&mut self) -> Result<()> {
        self.settings.validate()?;
        self.cancel_capture();
        ensure!(
            self.session.is_none(),
            "ゲーム中はゲーム画面で入力を確認してください。"
        );
        self.sampler = Some(
            self.bridge
                .as_ref()
                .context("入力ウィンドウがありません。")?
                .start_test(self.settings.input.clone())?,
        );
        self.test_events = 0;
        self.flashes = [None; 16];
        Ok(())
    }
    fn connect(&mut self) -> Result<()> {
        ensure!(self.session.is_none(), "既に接続中です。");
        self.settings.validate_connection()?;
        self.stop_test();
        self.cancel_capture();
        let session = Session::start(&self.settings, &self.path)?;
        self.last_log = Some(session.log.clone());
        self.session = Some(session);
        self.saved = serde_json::to_vec(&self.settings)?;
        self.error = None;
        self.status = "Switch 2 で募集を開始し、設定した表示名を承認してください。".into();
        Ok(())
    }
    fn disconnect(&mut self) -> Result<()> {
        if let Some(session) = &mut self.session {
            session.stop()?;
            self.status = "通信と音声を終了しています…".into();
        }
        Ok(())
    }
    fn open_log(&self) -> Result<()> {
        let log = self
            .last_log
            .clone()
            .or_else(|| {
                std::fs::read_dir(self.path.parent()?.join("logs"))
                    .ok()?
                    .flatten()
                    .map(|f| f.path())
                    .filter(|p| p.extension().is_some_and(|s| s == "log"))
                    .max()
            })
            .context("セッションログはまだありません。")?;
        Command::new("notepad.exe").arg(log).spawn()?;
        Ok(())
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.extraction {
            match job.result.try_recv() {
                Ok(result) => {
                    self.extraction = None;
                    match result {
                        Ok(path) => {
                            self.settings.passphrase_file = path;
                            self.status = "抽出しました。設定を保存してください。".into();
                            self.error = None;
                            self.extraction_open = false;
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.extraction = None;
                    self.error = Some("Extraction worker stopped unexpectedly".into());
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.bridge.is_some() {
            let focused = self.bridge.as_ref().is_some_and(Bridge::focused);
            if self.controller_capture.is_some() {
                if !focused {
                    self.cancel_capture();
                } else {
                    let key = self.bridge.as_ref().and_then(Bridge::take_key);
                    if key == Some(27) {
                        self.cancel_capture();
                    } else if key == Some(46) {
                        let cap = self.controller_capture.as_ref().unwrap();
                        let xinput = cap.xinput;
                        let target = cap.target;
                        let sony = self.sony_selected();
                        let result = crate::binding::assign(
                            &mut self.settings.input,
                            xinput,
                            target,
                            None,
                            sony,
                        );
                        self.cancel_capture();
                        self.record(result);
                    } else if let (Some(cap), Some(sampler)) =
                        (&mut self.controller_capture, &self.controller_sampler)
                    {
                        sampler.active.store(true, Ordering::Relaxed);
                        let snapshot = sampler.physical();
                        let physical = if cap.xinput {
                            snapshot.xinput
                        } else {
                            snapshot.directinput
                        };
                        if let Some(button) = cap.tracker.sample(physical) {
                            let result = crate::binding::assign(
                                &mut self.settings.input,
                                cap.xinput,
                                cap.target,
                                Some(button),
                                physical.vendor == 0x054c,
                            );
                            self.cancel_capture();
                            self.record(result);
                        }
                    }
                }
            }
            if let Some(target) = self.capture {
                if !focused {
                    self.cancel_capture();
                } else if let Some(key) = self.bridge.as_ref().and_then(Bridge::take_key) {
                    let result = assign(&mut self.settings, target, key).map(|changed| {
                        self.status = if changed {
                            "キーを変更しました。保存すると次の接続から適用されます。"
                        } else {
                            "キーの割り当てをキャンセルしました。"
                        }
                        .into();
                    });
                    self.cancel_capture();
                    self.record(result);
                }
            }
            if let Some(sampler) = &self.sampler {
                sampler.active.store(focused, Ordering::Relaxed);
                self.test_sample = sampler.latest();
                for (_, state) in sampler.events() {
                    self.test_events += 1;
                    for (i, flash) in self.flashes.iter_mut().enumerate() {
                        if state.buttons & (1 << i) != 0 {
                            *flash = Some(Instant::now());
                        }
                    }
                }
            }
        }
        if self.last_poll.elapsed() >= Duration::from_millis(300) {
            self.last_poll = Instant::now();
            let done = self.session.as_mut().map(|s| s.poll()).transpose();
            match done {
                Ok(Some(Some(done))) => {
                    self.session = None;
                    self.last_log = Some(done.log);
                    self.status = "切断しました。設定を変更して再接続できます。".into();
                    if !done.success {
                        self.error = Some(format!("接続が終了しました。\n{}", done.tail));
                    }
                    if self.closing {
                        if self.dirty() {
                            self.close_prompt = true;
                        } else {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
                Err(error) => self.error = Some(format!("{error:#}")),
                _ => {}
            }
        }
        ctx.request_repaint_after(
            if self.sampler.is_some()
                || self.capture.is_some()
                || self.controller_capture.is_some()
                || self.extraction.is_some()
            {
                Duration::from_millis(16)
            } else if self.session.is_some() {
                Duration::from_millis(300)
            } else {
                Duration::from_secs(2)
            },
        );
    }
    fn badge(&self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        let (name, color) = match &self.session {
            None => (tr("未接続"), self.palette.muted),
            Some(s) if s.stopping => (tr("切断中"), Color32::from_rgb(209, 142, 50)),
            Some(s) if s.playing => (tr("プレイ中"), Color32::from_rgb(45, 172, 125)),
            _ => (tr("接続中"), Color32::from_rgb(209, 142, 50)),
        };
        egui::Frame::new()
            .fill(self.palette.card)
            .stroke(Stroke::new(1.0, self.palette.border))
            .corner_radius(18)
            .inner_margin(egui::Margin::symmetric(12, 7))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, color);
                    ui.label(RichText::new(name).size(12.0).color(self.palette.text));
                });
            });
    }
    fn sidebar(&mut self, ctx: &egui::Context) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        egui::SidePanel::left("navigation")
            .exact_width(226.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(25, 29, 41))
                    .inner_margin(18),
            )
            .show(ctx, |ui| {
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 9, Color32::from_rgb(126, 109, 238));
                    let a = rect.min + egui::vec2(10.0, 24.0);
                    let b = rect.min + egui::vec2(10.0, 10.0);
                    let c = rect.min + egui::vec2(24.0, 24.0);
                    let d = rect.min + egui::vec2(24.0, 10.0);
                    ui.painter()
                        .line_segment([a, b], Stroke::new(2.3, Color32::WHITE));
                    ui.painter()
                        .line_segment([b, c], Stroke::new(2.3, Color32::WHITE));
                    ui.painter()
                        .line_segment([c, d], Stroke::new(2.3, Color32::WHITE));
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("NS2GameShare")
                                .size(16.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.label(
                            RichText::new("WINDOWS CLIENT")
                                .size(9.0)
                                .color(Color32::from_rgb(137, 150, 176)),
                        );
                    });
                });
                ui.add_space(14.0);
                egui::ComboBox::from_id_salt("ui_language")
                    .width(180.0)
                    .selected_text(match self.settings.language {
                        Language::Japanese => "日本語",
                        Language::English => "English",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.settings.language,
                            Language::Japanese,
                            "日本語",
                        );
                        ui.selectable_value(
                            &mut self.settings.language,
                            Language::English,
                            "English",
                        );
                    });
                ui.add_space(14.0);
                ui.label(
                    RichText::new("SETTINGS")
                        .size(10.0)
                        .strong()
                        .color(Color32::from_rgb(125, 138, 164)),
                );
                ui.add_space(10.0);
                for (i, (title, _)) in PAGES.iter().enumerate() {
                    let selected = self.page == i;
                    let text = RichText::new(format!("{:02}    {}", i + 1, tr(title)))
                        .size(14.0)
                        .color(if selected {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(166, 176, 198)
                        });
                    if ui
                        .add_sized(
                            [190.0, 47.0],
                            egui::Button::new(text)
                                .fill(if selected {
                                    Color32::from_rgb(68, 60, 111)
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .stroke(Stroke::NONE)
                                .corner_radius(8),
                        )
                        .clicked()
                    {
                        self.cancel_capture();
                        self.page = i;
                    }
                    ui.add_space(3.0);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        for (dark, label) in [(false, tr("ライト")), (true, tr("ダーク"))] {
                            let selected = self.settings.dark_theme == dark;
                            if ui
                                .add(
                                    egui::Button::new(RichText::new(label).size(12.0).color(
                                        if selected {
                                            Color32::WHITE
                                        } else {
                                            Color32::from_rgb(159, 171, 195)
                                        },
                                    ))
                                    .fill(if selected {
                                        Color32::from_rgb(60, 65, 84)
                                    } else {
                                        Color32::TRANSPARENT
                                    })
                                    .stroke(Stroke::NONE),
                                )
                                .clicked()
                            {
                                self.settings.dark_theme = dark;
                            }
                        }
                    });
                    ui.add_space(20.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(34, 39, 55))
                        .corner_radius(10)
                        .inner_margin(14)
                        .show(ui, |ui| {
                            ui.set_min_width(162.0);
                            ui.label(
                                RichText::new(if self.settings.port.starts_with("fx2") {
                                    "ESP32 + FX2LP"
                                } else {
                                    "ESP32 (Native USB)"
                                })
                                .size(12.0)
                                .strong()
                                .color(Color32::from_rgb(225, 230, 242)),
                            );
                            ui.label(
                                RichText::new(&self.settings.display_name)
                                    .size(11.0)
                                    .color(Color32::from_rgb(157, 169, 194)),
                            );
                        });
                    ui.add_space(6.0);
                });
            });
    }
    fn action_buttons(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        let active = self.session.is_some();
        let stopping = self.session.as_ref().is_some_and(|s| s.stopping);
        let title = if active {
            if stopping {
                tr("切断中…")
            } else {
                tr("切断")
            }
        } else {
            tr("保存して接続")
        };
        if ui
            .add_enabled(
                !stopping && self.extraction.is_none(),
                egui::Button::new(RichText::new(title).color(Color32::WHITE))
                    .fill(if active {
                        Color32::from_rgb(180, 74, 94)
                    } else {
                        self.palette.accent
                    })
                    .min_size(egui::vec2(142.0, 40.0))
                    .stroke(Stroke::NONE),
            )
            .clicked()
        {
            let result = if active {
                self.disconnect()
            } else {
                self.connect()
            };
            self.record(result);
        }
        if ui
            .add_enabled(
                self.dirty() || !self.path.exists(),
                egui::Button::new(tr("設定を保存")).min_size(egui::vec2(116.0, 40.0)),
            )
            .clicked()
        {
            let result = self.save_settings();
            self.record(result);
        }
    }
    fn status_text(&self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        ui.label(
            RichText::new(if self.dirty() {
                tr("● 未保存の変更があります")
            } else {
                tr("設定は保存されています")
            })
            .size(12.0)
            .color(if self.dirty() {
                self.palette.accent
            } else {
                self.palette.muted
            }),
        );
        ui.label(
            RichText::new(tr(&self.status))
                .size(11.0)
                .color(self.palette.muted),
        );
    }
    fn footer(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("actions")
            .frame(
                egui::Frame::new()
                    .fill(self.palette.card)
                    .inner_margin(egui::Margin::symmetric(28, 16))
                    .stroke(Stroke::new(1.0, self.palette.border)),
            )
            .show(ctx, |ui| {
                if ui.available_width() < 600.0 {
                    self.status_text(ui);
                    ui.horizontal(|ui| self.action_buttons(ui));
                } else {
                    ui.horizontal(|ui| {
                        let width = ui.available_width() - 300.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 44.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| self.status_text(ui),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.action_buttons(ui)
                        });
                    });
                }
            });
    }
    fn draw(&mut self, ctx: &egui::Context) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        if self.applied_theme != Some(self.settings.dark_theme) {
            theme(ctx, self.settings.dark_theme);
            self.palette = Palette::new(self.settings.dark_theme);
            self.applied_theme = Some(self.settings.dark_theme);
        }
        ctx.set_zoom_factor(self.settings.ui_scale);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(
            language.text("NS2GameShare — 設定").into(),
        ));
        self.sidebar(ctx);
        self.footer(ctx);
        let p = self.palette;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.background)
                    .inner_margin(egui::Margin::symmetric(32, 24)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("PREFERENCES")
                                .size(10.0)
                                .strong()
                                .color(p.muted),
                        );
                        ui.add_space(3.0);
                        ui.heading(tr(PAGES[self.page].0));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.badge(ui)
                    });
                });
                ui.label(
                    RichText::new(tr(PAGES[self.page].1))
                        .size(13.0)
                        .color(p.muted),
                );
                ui.add_space(18.0);
                if let Some(error) = self.error.clone() {
                    egui::Frame::new()
                        .fill(if self.settings.dark_theme {
                            Color32::from_rgb(67, 34, 44)
                        } else {
                            Color32::from_rgb(255, 238, 240)
                        })
                        .corner_radius(9)
                        .inner_margin(14)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(tr("入力・接続を確認してください"))
                                        .strong()
                                        .color(Color32::from_rgb(209, 75, 101)),
                                );
                                if ui.small_button(tr("閉じる")).clicked() {
                                    self.error = None;
                                }
                            });
                            egui::ScrollArea::vertical()
                                .max_height(112.0)
                                .show(ui, |ui| {
                                    ui.label(language.message(&error));
                                });
                        });
                    ui.add_space(12.0);
                }
                egui::ScrollArea::both()
                    .id_salt(("settings_scroll", self.page))
                    .auto_shrink([false, false])
                    .min_scrolled_width(0.0)
                    .show(ui, |ui| {
                        match self.page {
                            0 => self.connection_page(ui),
                            1 => self.media_page(ui),
                            2 => self.keyboard_page(ui),
                            3 => self.controller_page(ui),
                            4 => self.mouse_page(ui),
                            _ => self.test_page(ui),
                        }
                        ui.add_space(12.0);
                    });
            });
        if self.capture.is_some() {
            egui::Window::new(tr("キーを割り当て"))
                .id(egui::Id::new("capture"))
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(tr("使いたいキーを押してください。"));
                    ui.label(
                        RichText::new(tr("Esc: キャンセル   /   Delete: 未割当"))
                            .small()
                            .color(p.muted),
                    );
                    if ui.button(tr("キャンセル")).clicked() {
                        self.cancel_capture();
                    }
                });
        }
        if let Some(cap) = &self.controller_capture {
            let target = cap.target;
            egui::Window::new(tr("コントローラーの割り当て"))
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("{}: {}", tr("入力先"), tr(BUTTONS[target])));
                    ui.label(tr(
                        "すべてのボタンを離してから、割り当てるボタンを1つ押してください。",
                    ));
                    ui.label(tr("Esc: キャンセル   /   Delete: 未割当"));
                    if ui.button(tr("キャンセル")).clicked() {
                        self.cancel_capture();
                    }
                });
        }
        if self.extraction_open {
            self.extraction_dialog(ctx);
        }
        if self.close_prompt {
            egui::Window::new(tr("未保存の変更"))
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(tr("変更した設定を保存してから閉じますか？"));
                    ui.horizontal(|ui| {
                        if ui.button(tr("保存して閉じる")).clicked() {
                            let r = self.save_settings();
                            if r.is_ok() {
                                self.allow_close = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            self.record(r);
                        }
                        if ui.button(tr("保存せず閉じる")).clicked() {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button(tr("戻る")).clicked() {
                            self.close_prompt = false;
                            self.closing = false;
                        }
                    });
                });
        }
    }
    fn path_field(&mut self, ui: &mut egui::Ui, label: &str, which: usize) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).strong().size(13.0));
            if which == 1
                && ui
                    .add_enabled(
                        self.session.is_none(),
                        egui::Button::new(tr("SYSTEM / splay から抽出…")).small(),
                    )
                    .clicked()
            {
                self.cancel_capture();
                self.extraction_open = true;
            }
        });
        let path = match which {
            0 => &mut self.settings.keys,
            1 => &mut self.settings.passphrase_file,
            _ => &mut self.settings.fx2_firmware,
        };
        let mut value = path.to_string_lossy().into_owned();
        let mut browse = false;
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 105.0).max(120.0);
            if ui
                .add_sized(
                    [width, 36.0],
                    egui::TextEdit::singleline(&mut value)
                        .hint_text(tr("ファイルを選択してください")),
                )
                .changed()
            {
                *path = PathBuf::from(&value);
            }
            browse = ui
                .add_sized([90.0, 36.0], egui::Button::new(tr("参照…")))
                .clicked();
        });
        if browse {
            self.cancel_capture();
            if let Some(bridge) = &self.bridge {
                let path = match which {
                    0 => &mut self.settings.keys,
                    1 => &mut self.settings.passphrase_file,
                    _ => &mut self.settings.fx2_firmware,
                };
                let r = bridge.browse(path, language).map(|_| ());
                self.record(r);
            }
        }
        ui.add_space(8.0);
    }
    fn connection_page(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        let p = self.palette;
        card(
            ui,
            p,
            tr("参加プロフィール"),
            tr("Switch 2 の参加者一覧に表示されます。"),
            |ui| {
                row(ui, tr("表示名"), |ui| {
                    let width = (ui.available_width() - 70.0).max(120.0);
                    ui.add_sized(
                        [width, 36.0],
                        egui::TextEdit::singleline(&mut self.settings.display_name),
                    );
                    ui.label(
                        RichText::new(format!("{} / 32", self.settings.display_name.len()))
                            .size(12.0)
                            .color(
                                if crate::profile::validate_name(&self.settings.display_name)
                                    .is_err()
                                {
                                    Color32::from_rgb(210, 70, 95)
                                } else {
                                    p.muted
                                },
                            ),
                    );
                });
                ui.label(
                    RichText::new(tr(
                        "日本語は目安10文字まで。変更は次の接続から反映されます。",
                    ))
                    .small()
                    .color(p.muted),
                );
                ui.add_space(6.0);
                row(ui, tr("接続方法"), |ui| {
                    egui::ComboBox::from_id_salt("port")
                        .width((ui.available_width() - 95.0).clamp(120.0, 285.0))
                        .selected_text(match self.settings.port.as_str() {
                            "fx2" => "ESP32 + FX2LP",
                            "auto" => tr("ESP32 (Native USB) を自動検出"),
                            port => port,
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.settings.port,
                                "fx2".into(),
                                "ESP32 + FX2LP",
                            );
                            ui.selectable_value(
                                &mut self.settings.port,
                                "auto".into(),
                                tr("ESP32 (Native USB) を自動検出"),
                            );
                            for port in &self.ports {
                                ui.selectable_value(&mut self.settings.port, port.clone(), port);
                            }
                        });
                    if ui.button(tr("再検出")).clicked() {
                        let r = self.refresh();
                        self.record(r);
                    }
                });
                egui::CollapsingHeader::new(tr("接続先を直接指定")).show(ui, |ui| {
                    ui.text_edit_singleline(&mut self.settings.port);
                });
            },
        );
        card(
            ui,
            p,
            tr("認証ファイル"),
            tr("ファイルを参照します。鍵の内容を設定へ複製することはありません。"),
            |ui| {
                self.path_field(ui, "prod.keys", 0);
                self.path_field(ui, tr("パスフレーズファイル"), 1);
            },
        );
        if self.settings.port != "fx2" {
            self.settings.fx2_auto_load = false;
        }
        card(
            ui,
            p,
            tr("USB ブリッジ"),
            tr("ESP32 + FX2LP を使用する場合の接続準備です。"),
            |ui| {
                ui.add_enabled_ui(self.settings.port == "fx2", |ui| {
                    ui.checkbox(
                        &mut self.settings.fx2_auto_load,
                        tr("接続時に FX2 の RAM ファームを自動ロードする"),
                    );
                });
                if self.settings.fx2_auto_load {
                    self.path_field(ui, tr("FX2 RAM ファーム"), 2);
                }
                ui.label(
                    RichText::new(tr("RAM への一時ロードです。EEPROM は変更しません。"))
                        .small()
                        .color(p.muted),
                );
            },
        );
        ui.horizontal(|ui| {
            if ui.button(tr("セッションログを開く")).clicked() {
                let r = self.open_log();
                self.record(r);
            }
            ui.label(
                RichText::new(tr("接続に問題がある場合の確認に使用できます。"))
                    .small()
                    .color(p.muted),
            );
        });
    }
    fn media_page(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        let p = self.palette;
        card(
            ui,
            p,
            tr("映像"),
            tr("ホストが送信する映像の希望設定です。"),
            |ui| {
                row(ui, tr("要求解像度"), |ui| {
                    egui::ComboBox::from_id_salt("resolution")
                        .width(250.0)
                        .selected_text(format!(
                            "{} × {}",
                            self.settings.video_width, self.settings.video_height
                        ))
                        .show_ui(ui, |ui| {
                            for (w, h) in [(640, 360), (960, 540), (1280, 720), (1920, 1080)] {
                                if ui
                                    .selectable_label(
                                        (self.settings.video_width, self.settings.video_height)
                                            == (w, h),
                                        format!("{w} × {h}"),
                                    )
                                    .clicked()
                                {
                                    self.settings.video_width = w;
                                    self.settings.video_height = h;
                                }
                            }
                        });
                });
                row(ui, tr("受信容量の上限"), |ui| {
                    ui.add(
                        egui::Slider::new(&mut self.settings.receive_kbps, 600..=2000)
                            .suffix(" kbps"),
                    );
                });
                ui.checkbox(&mut self.settings.denoise, tr("GPU ノイズ低減を有効にする"));
                ui.label(RichText::new(tr("解像度はホストが最終決定します。ノイズ低減はプレイ中に F9 でも切り替えられます。")).small().color(p.muted));
            },
        );
        card(
            ui,
            p,
            tr("音声と同期"),
            tr("低遅延でのプレイを優先する場合は、映像同期をオフにします。"),
            |ui| {
                ui.checkbox(&mut self.settings.audio, tr("ゲーム音を再生する"));
                ui.add_enabled_ui(self.settings.audio, |ui| {
                    ui.checkbox(
                        &mut self.settings.av_sync,
                        tr("映像を音声の再生時刻に合わせる"),
                    );
                });
                ui.label(
                    RichText::new(tr(
                        "映像同期を有効にすると、操作から表示までの待ち時間が増えます。",
                    ))
                    .small()
                    .color(p.muted),
                );
            },
        );
        card(
            ui,
            p,
            tr("設定画面の表示"),
            tr("再生画面の解像度には影響しません。"),
            |ui| {
                row(ui, tr("テーマ"), |ui| {
                    ui.selectable_value(&mut self.settings.dark_theme, false, tr("ライト"));
                    ui.selectable_value(&mut self.settings.dark_theme, true, tr("ダーク"));
                });
                row(ui, tr("表示倍率"), |ui| {
                    let mut percent = self.settings.ui_scale * 100.0;
                    if ui
                        .add(
                            egui::Slider::new(&mut percent, 80.0..=150.0)
                                .suffix(" %")
                                .step_by(5.0),
                        )
                        .changed()
                    {
                        self.settings.ui_scale = percent / 100.0;
                    }
                });
            },
        );
    }
    fn bind_button(&mut self, ui: &mut egui::Ui, label: &str, target: Binding, key: Option<u16>) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        ui.horizontal(|ui| {
            ui.add_sized([118.0, 36.0], egui::Label::new(tr(label)));
            let waiting = self.capture == Some(target);
            if ui
                .add_sized(
                    [ui.available_width().max(92.0), 36.0],
                    egui::Button::new(
                        RichText::new(if waiting {
                            tr("キーを押してください…").into()
                        } else {
                            key_name(key, language)
                        })
                        .color(if waiting {
                            self.palette.accent
                        } else {
                            self.palette.text
                        }),
                    ),
                )
                .clicked()
            {
                self.start_capture(target);
            }
        });
    }
    fn keyboard_page(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        let p = self.palette;
        ui.horizontal(|ui| {
            if ui.button(tr("既定のキー配置に戻す")).clicked() {
                self.settings.input.keyboard = Default::default();
            }
            if ui.button(tr("WASD プリセット")).clicked() {
                let mut k = crate::input::Keyboard {
                    left_stick: [Some(65), Some(87), Some(68), Some(83)],
                    ..Default::default()
                };
                for (i, v) in [(0, 76), (1, 75), (2, 73), (3, 74), (7, 69), (9, 51)] {
                    k.buttons[i] = Some(v);
                }
                self.settings.input.keyboard = k;
            }
        });
        ui.add_space(14.0);
        card(
            ui,
            p,
            tr("ゲームのボタン"),
            tr("クリック → キー押下で変更。Delete で未割当、Esc でキャンセルします。"),
            |ui| {
                let count = if ui.available_width() < 560.0 { 1 } else { 2 };
                ui.columns(count, |columns| {
                    for (i, label) in BUTTONS.iter().enumerate() {
                        let col = if count == 1 { 0 } else { i / 8 };
                        let key = self.settings.input.keyboard.buttons[i];
                        columns[col]
                            .push_id(i, |ui| self.bind_button(ui, label, Binding::Button(i), key));
                    }
                });
            },
        );
        card(
            ui,
            p,
            tr("左スティック"),
            tr("十字キーとは別に割り当てられます。"),
            |ui| {
                let count = if ui.available_width() < 560.0 { 1 } else { 2 };
                ui.columns(count, |columns| {
                    for (i, label) in [tr("左 ←"), tr("上 ↑"), tr("右 →"), tr("下 ↓")]
                        .iter()
                        .enumerate()
                    {
                        let key = self.settings.input.keyboard.left_stick[i];
                        columns[if count == 1 { 0 } else { i / 2 }].push_id(("stick", i), |ui| {
                            self.bind_button(ui, label, Binding::Stick(i), key)
                        });
                    }
                });
            },
        );
        ui.label(RichText::new(tr("Esc はプレイ終了、F9 はノイズ低減用です。マウス取り込みキーとの重複は保存時に確認します。")).small().color(p.muted));
    }
    fn sony_selected(&self) -> bool {
        self.settings.input.directinput.device.as_ref().map_or_else(
            || self.controllers.iter().any(|d| d.vendor == 0x054c),
            |id| {
                self.controllers
                    .iter()
                    .any(|d| d.id == *id && d.vendor == 0x054c)
            },
        )
    }
    fn default_map(&self) -> [Option<u8>; 16] {
        if self.map_source == 1 {
            return crate::input::XINPUT_DEFAULT.map(Some);
        }
        let values = if self.sony_selected() {
            [3, 1, 0, 2, 6, 7, 8, 9, 11, 10, 4, 5]
        } else {
            [1, 0, 3, 2, 6, 7, 8, 9, 11, 10, 4, 5]
        };
        std::array::from_fn(|i| values.get(i).copied())
    }
    fn start_controller_capture(&mut self, target: usize) -> Result<()> {
        self.stop_test();
        self.cancel_capture();
        ensure!(
            self.session.is_none(),
            "コントローラーの割り当て前に切断してください。"
        );
        let xinput = self.map_source == 1;
        let mut config = self.settings.input.clone();
        config.mouse_enabled = false;
        config.keyboard.buttons = [None; 16];
        config.keyboard.left_stick = [None; 4];
        config.directinput.enabled = !xinput;
        config.xinput_enabled = xinput;
        let bridge = self
            .bridge
            .as_ref()
            .context("入力ウィンドウを利用できません。")?;
        self.controller_sampler = Some(bridge.start_test(config)?);
        bridge.begin_capture();
        self.controller_capture = Some(ControllerCapture {
            xinput,
            target,
            tracker: Default::default(),
        });
        Ok(())
    }
    fn controller_label(&self, target: usize) -> String {
        let tr = |s| self.settings.language.text(s);
        let map: Vec<Option<u8>> = if self.map_source == 1 {
            self.settings
                .input
                .xinput_buttons
                .unwrap_or_else(|| crate::input::XINPUT_DEFAULT.map(Some))
                .to_vec()
        } else {
            self.settings
                .input
                .directinput
                .buttons
                .clone()
                .unwrap_or_else(|| self.default_map().to_vec())
        };
        let xbox = [
            "A",
            "B",
            "X",
            "Y",
            "L3",
            "R3",
            "LB",
            "RB",
            "Start",
            "Back",
            "D-pad Left",
            "D-pad Up",
            "D-pad Right",
            "D-pad Down",
            "LT",
            "RT",
        ];
        let mut labels: Vec<String> = map
            .iter()
            .enumerate()
            .filter(|(_, v)| **v == Some(target as u8))
            .map(|(i, _)| {
                if self.map_source == 1 {
                    format!("XInput {}", xbox.get(i).unwrap_or(&"?"))
                } else {
                    format!("{} {}", tr("ボタン"), i + 1)
                }
            })
            .collect();
        if self.map_source == 0 {
            for (i, v) in self
                .settings
                .input
                .directinput
                .pov_buttons
                .iter()
                .enumerate()
            {
                if *v == Some(target as u8) {
                    labels.push(format!("POV {}", ["←", "↑", "→", "↓"][i]));
                }
            }
        }
        if labels.is_empty() {
            tr("未割当").into()
        } else {
            labels.join(", ")
        }
    }
    fn controller_page(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        let p = self.palette;
        ui.horizontal(|ui| {
            for (id, label) in [(0, "DirectInput"), (1, "XInput")] {
                if ui
                    .selectable_value(&mut self.map_source, id, label)
                    .changed()
                {
                    self.cancel_capture();
                }
            }
        });
        ui.add_space(12.0);
        card(
            ui,
            p,
            if self.map_source == 0 {
                "DirectInput"
            } else {
                "XInput"
            },
            tr("入力方式ごとにデバイスとスティックを設定します。"),
            |ui| {
                if self.map_source == 0 {
                    ui.checkbox(
                        &mut self.settings.input.directinput.enabled,
                        tr("DirectInputを有効にする"),
                    );
                    let label = self
                        .settings
                        .input
                        .directinput
                        .device
                        .as_ref()
                        .and_then(|id| self.controllers.iter().find(|d| d.id == *id))
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| {
                            if self.settings.input.directinput.device.is_some() {
                                tr("保存済みデバイス（未接続）").into()
                            } else {
                                tr("自動選択").into()
                            }
                        });
                    row(ui, tr("デバイス"), |ui| {
                        egui::ComboBox::from_id_salt("controller")
                            .width((ui.available_width() - 95.0).clamp(120.0, 280.0))
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.settings.input.directinput.device,
                                    None,
                                    tr("自動選択"),
                                );
                                for d in &self.controllers {
                                    ui.selectable_value(
                                        &mut self.settings.input.directinput.device,
                                        Some(d.id.clone()),
                                        &d.name,
                                    );
                                }
                            });
                        if ui.button(tr("再検出")).clicked() {
                            let result = self.refresh();
                            self.record(result);
                        }
                    });
                    row(ui, tr("デッドゾーン"), |ui| {
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings.input.directinput.deadzone,
                                0.0..=0.6,
                            )
                            .custom_formatter(|v, _| format!("{:.0} %", v * 100.0)),
                        );
                    });
                    egui::CollapsingHeader::new(tr("軸と反転")).show(ui, |ui| {
                        for (i, label) in [
                            "左スティック X",
                            "左スティック Y",
                            "右スティック X",
                            "右スティック Y",
                        ]
                        .iter()
                        .enumerate()
                        {
                            row(ui, tr(label), |ui| {
                                egui::ComboBox::from_id_salt(("axis", i))
                                    .width(120.0)
                                    .selected_text(AXES[self.settings.input.directinput.axes[i]])
                                    .show_ui(ui, |ui| {
                                        for (n, axis) in AXES.iter().enumerate() {
                                            ui.selectable_value(
                                                &mut self.settings.input.directinput.axes[i],
                                                n,
                                                *axis,
                                            );
                                        }
                                    });
                                ui.checkbox(
                                    &mut self.settings.input.directinput.invert[i],
                                    tr("反転"),
                                );
                            });
                        }
                    });
                } else {
                    ui.checkbox(
                        &mut self.settings.input.xinput_enabled,
                        tr("XInput を有効にする"),
                    );
                    row(ui, tr("デバイス"), |ui| {
                        let selected = self
                            .settings
                            .input
                            .xinput_index
                            .map_or_else(|| tr("自動選択").into(), |i| format!("XInput {}", i + 1));
                        egui::ComboBox::from_id_salt("xinput_slot")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.settings.input.xinput_index,
                                    None,
                                    tr("自動選択"),
                                );
                                for i in 0..4 {
                                    ui.selectable_value(
                                        &mut self.settings.input.xinput_index,
                                        Some(i),
                                        format!("XInput {}", i + 1),
                                    );
                                }
                            });
                    });
                    for (i, label) in ["XInput 左 DZ", "XInput 右 DZ"].iter().enumerate() {
                        row(ui, tr(label), |ui| {
                            ui.add(
                                egui::Slider::new(
                                    &mut self.settings.input.xinput_deadzone[i],
                                    0.0..=0.6,
                                )
                                .custom_formatter(|v, _| format!("{:.0} %", v * 100.0)),
                            );
                        });
                    }
                }
            },
        );
        card(
            ui,
            p,
            tr("ボタンの割り当て"),
            tr("入力先をクリックし、実コントローラーのボタンを押して割り当てます。"),
            |ui| {
                ui.label(tr(
                    "Escで取消、Deleteで解除。十字キーとXInputのトリガーにも対応します。",
                ));
                let count = if ui.available_width() < 560.0 { 1 } else { 2 };
                ui.columns(count, |columns| {
                    for (target, label) in BUTTONS.iter().enumerate() {
                        columns[if count == 1 { 0 } else { target / 8 }].push_id(
                            ("controller_binding", target),
                            |ui| {
                                ui.horizontal(|ui| {
                                    ui.add_sized([118.0, 36.0], egui::Label::new(tr(label)));
                                    let text = self.controller_label(target);
                                    if ui
                                        .add_enabled(
                                            self.session.is_none(),
                                            egui::Button::new(text).min_size(egui::vec2(
                                                ui.available_width().max(92.0),
                                                36.0,
                                            )),
                                        )
                                        .clicked()
                                    {
                                        let result = self.start_controller_capture(target);
                                        self.record(result);
                                    }
                                });
                            },
                        );
                    }
                });
            },
        );
        if ui.button(tr("この方式の設定を既定に戻す")).clicked() {
            self.cancel_capture();
            let defaults = crate::input::Config::default();
            if self.map_source == 0 {
                self.settings.input.directinput = defaults.directinput;
            } else {
                self.settings.input.xinput_enabled = defaults.xinput_enabled;
                self.settings.input.xinput_index = defaults.xinput_index;
                self.settings.input.xinput_deadzone = defaults.xinput_deadzone;
                self.settings.input.xinput_buttons = None;
            }
        }
    }
    fn mouse_page(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        let p = self.palette;
        card(
            ui,
            p,
            tr("マウスで視点を操作"),
            tr("右スティックまたは実験的ジャイロへマウス移動を割り当てます。"),
            |ui| {
                ui.checkbox(
                    &mut self.settings.input.mouse_enabled,
                    tr("マウス操作を有効にする"),
                );
                self.bind_button(
                    ui,
                    tr("取り込み / 解除"),
                    Binding::Mouse,
                    Some(self.settings.input.mouse_capture_key),
                );
                row(ui, tr("割り当て先"), |ui| {
                    ui.selectable_value(
                        &mut self.settings.input.mouse_mode,
                        crate::motion::MouseMode::RightStick,
                        tr("右スティック"),
                    );
                    ui.selectable_value(
                        &mut self.settings.input.mouse_mode,
                        crate::motion::MouseMode::Gyro,
                        tr("ジャイロ（実験的）"),
                    );
                });
                if self.settings.input.mouse_mode == crate::motion::MouseMode::RightStick {
                    row(ui, tr("感度"), |ui| {
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings.input.mouse_sensitivity,
                                0.0001..=1.0,
                            )
                            .logarithmic(true)
                            .max_decimals(4),
                        );
                    });
                    row(ui, tr("平滑化"), |ui| {
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings.input.mouse_smoothing_ms,
                                1.0..=100.0,
                            )
                            .suffix(" ms"),
                        );
                    });
                    ui.label(RichText::new(tr("感度は高いほど少ない移動で大きく動きます。平滑化は低いほど応答が速くなります。")).small().color(p.muted));
                } else {
                    ui.label(tr(
                        "ゲーム側のジャイロ対応が必要です。実ゲームでの動作は未検証です。",
                    ));
                    for (pitch, label) in [
                        (false, "横方向：1回転のマウスカウント"),
                        (true, "縦方向：1回転のマウスカウント"),
                    ] {
                        let value = if pitch {
                            &mut self.settings.input.mouse_gyro.pitch_counts_per_turn
                        } else {
                            &mut self.settings.input.mouse_gyro.yaw_counts_per_turn
                        };
                        row(ui, tr(label), |ui| {
                            ui.add(
                                egui::Slider::new(value, 500.0..=200000.0)
                                    .logarithmic(true)
                                    .max_decimals(0),
                            );
                        });
                    }
                    ui.label(tr("カウント値が小さいほど高感度になります。"));
                    row(ui, tr("上下の可動範囲"), |ui| {
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings.input.mouse_gyro.pitch_limit_deg,
                                5.0..=89.0,
                            )
                            .suffix(" °"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            &mut self.settings.input.mouse_gyro.invert_yaw,
                            tr("横方向を反転"),
                        );
                        ui.checkbox(
                            &mut self.settings.input.mouse_gyro.invert_pitch,
                            tr("縦方向を反転"),
                        );
                    });
                    self.bind_button(
                        ui,
                        tr("姿勢をリセット"),
                        Binding::GyroReset,
                        Some(self.settings.input.mouse_gyro.reset_key),
                    );
                }
            },
        );
        card(
            ui,
            p,
            tr("マウスのボタン"),
            tr("取り込み中だけゲームに送信します。"),
            |ui| {
                for (i, label) in [
                    tr("左ボタン"),
                    tr("右ボタン"),
                    tr("中央ボタン"),
                    tr("戻るボタン"),
                    tr("進むボタン"),
                ]
                .iter()
                .enumerate()
                {
                    row(ui, label, |ui| {
                        target_combo(
                            language,
                            ui,
                            ("mouse", i),
                            &mut self.settings.input.mouse_buttons[i],
                            false,
                        );
                    });
                }
            },
        );
        ui.label(RichText::new(tr("フォーカスを外すと取り込みを解除します。設定変更後は入力テストを再開してください。")).small().color(p.muted));
    }
    fn test_page(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language;
        let tr = |s| language.text(s);
        let p = self.palette;
        card(
            ui,
            p,
            tr("入力の確認"),
            tr(
                "テスト中の操作はゲームへ送信されません。設定を変更したらテストを再開してください。",
            ),
            |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.session.is_none(),
                            egui::Button::new(if self.sampler.is_some() {
                                tr("テストを停止")
                            } else {
                                tr("入力テストを開始")
                            }),
                        )
                        .clicked()
                    {
                        if self.sampler.is_some() {
                            self.stop_test();
                        } else {
                            let r = self.start_test();
                            self.record(r);
                        }
                    }
                    ui.label(
                        RichText::new(format!("{}: {}", tr("入力変化"), self.test_events))
                            .color(p.muted),
                    );
                });
                if let Some(motion) = self.test_sample.motion {
                    ui.label(format!(
                        "Gyro yaw: {:.2}°   pitch: {:.2}°   {}",
                        motion.yaw * 360.0,
                        motion.pitch * 360.0,
                        if motion.active {
                            tr("取り込み中")
                        } else {
                            tr("解除中")
                        }
                    ));
                }
                if self.session.is_some() {
                    ui.label(tr(
                        "プレイ中はゲーム画面をクリックして操作を確認してください。",
                    ));
                }
            },
        );
        card(
            ui,
            p,
            tr("スティック"),
            tr("中心からの入力をリアルタイムで表示します。"),
            |ui| {
                if ui.available_width() < 400.0 {
                    stick(
                        ui,
                        p,
                        tr("左スティック"),
                        self.test_sample.lx,
                        self.test_sample.ly,
                    );
                    stick(
                        ui,
                        p,
                        tr("右スティック"),
                        self.test_sample.rx,
                        self.test_sample.ry,
                    );
                } else {
                    ui.columns(2, |columns| {
                        stick(
                            &mut columns[0],
                            p,
                            tr("左スティック"),
                            self.test_sample.lx,
                            self.test_sample.ly,
                        );
                        stick(
                            &mut columns[1],
                            p,
                            tr("右スティック"),
                            self.test_sample.rx,
                            self.test_sample.ry,
                        );
                    });
                }
            },
        );
        card(
            ui,
            p,
            tr("ボタン"),
            tr("短い押下も見つけやすいよう、ハイライトを少し保持します。"),
            |ui| {
                let count = ((ui.available_width() / 122.0).floor() as usize).clamp(1, 4);
                egui::Grid::new("input_buttons")
                    .num_columns(count)
                    .spacing([12.0, 12.0])
                    .show(ui, |ui| {
                        for (i, label) in BUTTONS.iter().enumerate() {
                            let on = self.test_sample.buttons & (1 << i) != 0
                                || self.flashes[i].is_some_and(|time| {
                                    time.elapsed() < Duration::from_millis(250)
                                });
                            ui.add_sized(
                                [110.0, 36.0],
                                egui::Button::new(RichText::new(tr(label)).color(if on {
                                    Color32::WHITE
                                } else {
                                    p.muted
                                }))
                                .fill(if on { p.accent } else { p.input })
                                .stroke(Stroke::new(1.0, if on { p.accent } else { p.border }))
                                .sense(egui::Sense::hover()),
                            );
                            if i % count == count - 1 {
                                ui.end_row();
                            }
                        }
                    });
            },
        );
    }
}
fn target_combo(
    language: Language,
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    target: &mut Option<u8>,
    extended: bool,
) {
    let tr = |s| language.text(s);
    let name = target.map_or_else(
        || tr("未割当").into(),
        |i| {
            BUTTONS
                .get(i as usize)
                .map_or_else(|| format!("{} {i}", tr("拡張")), |name| (*name).into())
        },
    );
    egui::ComboBox::from_id_salt(id)
        .width(118.0)
        .selected_text(name)
        .show_ui(ui, |ui| {
            ui.selectable_value(target, None, tr("未割当"));
            for (i, label) in BUTTONS.iter().enumerate() {
                ui.selectable_value(target, Some(i as u8), *label);
            }
            if extended {
                egui::CollapsingHeader::new(tr("拡張ボタン")).show(ui, |ui| {
                    for i in 16..32 {
                        ui.selectable_value(target, Some(i), format!("{} {i}", tr("拡張")));
                    }
                });
            }
        });
}
fn stick(ui: &mut egui::Ui, p: Palette, label: &str, x: f32, y: f32) {
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(label).strong());
        let (rect, _) = ui.allocate_exact_size(egui::vec2(168.0, 168.0), egui::Sense::hover());
        let center = rect.center();
        let radius = 67.0;
        let painter = ui.painter();
        painter.circle(center, radius, p.input, Stroke::new(1.0, p.border));
        painter.circle_stroke(center, radius * 0.2, Stroke::new(1.0, p.border));
        painter.line_segment(
            [
                center - egui::vec2(radius, 0.0),
                center + egui::vec2(radius, 0.0),
            ],
            Stroke::new(1.0, p.border),
        );
        painter.line_segment(
            [
                center - egui::vec2(0.0, radius),
                center + egui::vec2(0.0, radius),
            ],
            Stroke::new(1.0, p.border),
        );
        let point = center + egui::vec2(x, -y) * radius;
        painter.line_segment([center, point], Stroke::new(2.0, p.accent));
        painter.circle_filled(point, 8.0, p.accent);
        ui.label(
            RichText::new(format!("X {x:+.2}   Y {y:+.2}"))
                .monospace()
                .small()
                .color(p.muted),
        );
    });
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            if self.session.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.closing = true;
                let r = self.disconnect();
                self.record(r);
            } else if self.dirty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.close_prompt = true;
            }
        }
        self.poll(ctx);
        if self.capture.is_none()
            && self.controller_capture.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S))
        {
            let r = self.save_settings();
            self.record(r);
        }
        self.draw(ctx);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_test();
        self.cancel_capture();
        if let Some(s) = &mut self.session {
            let _ = s.stop();
        }
    }
}
pub fn run(path: Option<PathBuf>) -> Result<()> {
    let result = (|| -> Result<()> {
        let path = path.unwrap_or_else(default_path);
        let settings =
            Settings::load(&path).with_context(|| format!("Settings file: {}", path.display()))?;
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_icon(std::sync::Arc::new(app_icon()))
                .with_inner_size([1120.0, 820.0])
                .with_min_inner_size([900.0, 640.0])
                .with_title("NS2GameShare — 設定"),
            renderer: eframe::Renderer::Glow,
            centered: true,
            ..Default::default()
        };
        eframe::run_native(
            "NS2GameShare",
            options,
            Box::new(move |cc| {
                fonts(&cc.egui_ctx);
                let bridge = Bridge::new(cc)?;
                let mut app = App::new(settings, path, Some(bridge));
                let r = app.refresh();
                app.record(r);
                Ok(Box::new(app))
            }),
        )
        .map_err(|error| anyhow::anyhow!("Unable to open settings: {error}"))?;
        Ok(())
    })();
    if let Err(error) = &result {
        native::show_error(&Language::English.message(&format!("{error:#}")));
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extraction_dialog_supports_both_languages_without_hardware() {
        for language in [Language::Japanese, Language::English] {
            let ctx = egui::Context::default();
            let mut app = App::new(
                Settings {
                    language,
                    ..Default::default()
                },
                PathBuf::from("test.json"),
                None,
            );
            app.extraction_open = true;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1120.0, 820.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.draw(ctx),
            );
            assert!(!output.shapes.is_empty());
            assert!(app.extraction_open);
            assert!(app.extraction.is_none());
        }
    }
    #[test]
    fn native_capture_preserves_cancel_delete_and_utf8_settings() {
        let mut s = Settings {
            display_name: "テスト PC".into(),
            ..Default::default()
        };
        assert!(assign(&mut s, Binding::Button(0), 67).unwrap());
        assert_eq!(s.input.keyboard.buttons[0], Some(67));
        assert!(!assign(&mut s, Binding::Button(0), 27).unwrap());
        assert_eq!(s.input.keyboard.buttons[0], Some(67));
        assert!(assign(&mut s, Binding::Button(0), 46).unwrap());
        assert_eq!(s.input.keyboard.buttons[0], None);
        assert!(assign(&mut s, Binding::Mouse, 46).is_err());
        assert!(assign(&mut s, Binding::Stick(0), 0x78).is_err());
        s.dark_theme = true;
        s.ui_scale = 1.15;
        s.validate().unwrap();
        let restored: Settings = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(restored.display_name, "テスト PC");
        assert!(restored.dark_theme);
    }
    #[test]
    fn mixed_japanese_and_latin_share_font_metrics_when_japanese_fonts_are_installed() {
        let ctx = egui::Context::default();
        fonts(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            ctx.fonts_mut(|fonts| {
                let id = egui::FontId::proportional(14.0);
                if !fonts.has_glyph(&id, '日') {
                    return;
                }
                let galley =
                    fonts.layout_no_wrap("ESP32日本語DirectInput".into(), id, Color32::WHITE);
                let glyphs = &galley.rows[0].glyphs;
                let japanese = glyphs.iter().find(|g| g.chr == '日').unwrap();
                for glyph in glyphs.iter().filter(|g| g.chr.is_ascii_alphanumeric()) {
                    assert!((glyph.font_impl_ascent - japanese.font_impl_ascent).abs() < 0.01);
                    assert!((glyph.font_impl_height - japanese.font_impl_height).abs() < 0.01);
                }
            });
        });
    }
    #[test]
    fn every_page_renders_at_minimum_window_size_without_native_hardware() {
        fn english(shape: &egui::epaint::Shape) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    let text = t.galley.job.text.replace("日本語", "");
                    assert!(
                        !text
                            .chars()
                            .any(|c| (0x3040..=0x9fff).contains(&(c as u32))),
                        "Untranslated: {text}"
                    );
                }
                egui::epaint::Shape::Vec(v) => {
                    for s in v {
                        english(s)
                    }
                }
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        let mut app = App::new(
            Settings::default(),
            PathBuf::from("settings-test.json"),
            None,
        );
        app.settings.input.directinput.buttons = Some(vec![None; 128]);
        app.settings.input.xinput_buttons = Some(crate::input::XINPUT_DEFAULT.map(Some));
        for language in [Language::Japanese, Language::English] {
            app.settings.language = language;
            for dark in [false, true] {
                app.settings.dark_theme = dark;
                for width in [900.0, 650.0] {
                    for variant in 0..2 {
                        app.map_source = variant;
                        app.settings.input.mouse_mode = if variant == 0 {
                            crate::motion::MouseMode::RightStick
                        } else {
                            crate::motion::MouseMode::Gyro
                        };
                        for page in 0..PAGES.len() {
                            app.page = page;
                            app.controller_capture = if page == 3 && variant == 1 {
                                Some(ControllerCapture {
                                    xinput: true,
                                    target: 0,
                                    tracker: Default::default(),
                                })
                            } else {
                                None
                            };
                            let mut used = egui::Rect::NOTHING;
                            let out = ctx.run(
                                egui::RawInput {
                                    screen_rect: Some(egui::Rect::from_min_size(
                                        egui::Pos2::ZERO,
                                        egui::vec2(width, 640.0),
                                    )),
                                    ..Default::default()
                                },
                                |ctx| {
                                    app.draw(ctx);
                                    used = ctx.used_rect();
                                },
                            );
                            assert!(!out.shapes.is_empty());
                            if language == Language::English {
                                for s in &out.shapes {
                                    english(&s.shape);
                                }
                            }
                            assert!(
                                used.is_finite() && used.right() <= width + 1.0,
                                "Page {page}, {language:?} at {width}: {used:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
