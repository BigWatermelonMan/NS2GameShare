//! Media Foundation -> D3D11 NV12 texture -> GPU video processor -> flip swapchain.
//! File playback validates the native presentation path independently of LSP transport.
use crate::input::Sampler;
static DENOISE: AtomicBool = AtomicBool::new(false);
use std::sync::{
    Arc, Mutex,
    atomic::AtomicBool,
    mpsc::{Receiver, TryRecvError},
};
pub struct Live {
    pub receiver: Receiver<crate::lsp::MediaUnit>,
    pub controls: Arc<Mutex<crate::feedback::Controls>>,
    pub stop: Arc<AtomicBool>,
    pub input_config: crate::input::Config,
    pub timeline: Option<Arc<Mutex<crate::timeline::Timeline>>>,
    pub denoise: bool,
}
use anyhow::{Context, Result, ensure};
use std::{
    io::Write,
    mem::ManuallyDrop,
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
        Media::MediaFoundation::*,
        System::{Com::*, LibraryLoader::GetModuleHandleW},
        UI::{Input::KeyboardAndMouse::GetAsyncKeyState, WindowsAndMessaging::*},
    },
    core::{Interface, PCWSTR, w},
};
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_SYSKEYDOWN if w.0 == 0x73 => DefWindowProcW(hwnd, msg, w, l), // Alt+F4
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if w.0 == 27 {
                    crate::mouse::release();
                    let _ = DestroyWindow(hwnd);
                } else if w.0 != 0 && w.0 == crate::input::mouse_capture_key() as usize {
                    if l.0 & (1 << 30) == 0 {
                        let _ = crate::mouse::toggle(hwnd);
                    }
                } else if w.0 == 0x78 {
                    if l.0 & (1 << 30) == 0 {
                        DENOISE.fetch_xor(true, Ordering::Relaxed);
                    }
                } else {
                    crate::input::key_down(w.0 as u32);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                crate::mouse::release();
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                crate::mouse::release();
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                crate::ds4::clear();
                crate::mouse::release();
                DefWindowProcW(hwnd, msg, w, l)
            }
            WM_INPUT => {
                crate::ds4::raw(windows::Win32::UI::Input::HRAWINPUT(l.0 as *mut _));
                crate::mouse::raw(windows::Win32::UI::Input::HRAWINPUT(l.0 as *mut _));
                DefWindowProcW(hwnd, msg, w, l)
            }
            WM_INPUT_DEVICE_CHANGE => {
                crate::ds4::changed();
                DefWindowProcW(hwnd, msg, w, l)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}
struct Runtime;
impl Runtime {
    fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                CoUninitialize();
                return Err(e.into());
            }
        }
        Ok(Self)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}
struct Window(HWND);
impl Drop for Window {
    fn drop(&mut self) {
        crate::mouse::release();
        unsafe {
            if IsWindow(Some(self.0)).as_bool() {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}
pub fn preview(path: &Path, seconds: u64, input_log: Option<&Path>) -> Result<()> {
    run(Some(path), None, seconds, input_log)
}
pub fn live(source: Live, seconds: u64, input_log: Option<&Path>) -> Result<()> {
    let stop = source.stop.clone();
    let result = run(None, Some(source), seconds, input_log);
    stop.store(true, Ordering::Relaxed);
    result
}
fn run(
    path: Option<&Path>,
    mut live: Option<Live>,
    seconds: u64,
    input_log: Option<&Path>,
) -> Result<()> {
    let _runtime = Runtime::new()?;
    DENOISE.store(
        live.as_ref().is_some_and(|source| source.denoise),
        Ordering::Relaxed,
    );
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("NS2GameShareVideo");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        ensure!(RegisterClassW(&wc) != 0, "RegisterClassW failed");
        let window = Window(CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("NS2GameShare"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280,
            760,
            None,
            None,
            Some(instance.into()),
            None,
        )?);
        // WS_VISIBLE's initial show can inherit SW_HIDE from a launcher that
        // hides the console. Explicitly show the independently owned game UI.
        let _ = ShowWindow(window.0, SW_SHOW);
        crate::ds4::register(window.0)?;
        let mut device = None;
        let mut context = None;
        let mut swap = None;
        let desc = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC {
                Width: 1280,
                Height: 720,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                ..Default::default()
            },
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            OutputWindow: window.0,
            Windowed: true.into(),
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
        };
        D3D11CreateDeviceAndSwapChain(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&desc),
            Some(&mut swap),
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let device = device.context("D3D11 device missing")?;
        let context = context.context("D3D11 context missing")?;
        let swap = swap.context("swapchain missing")?;
        let swap2: IDXGISwapChain2 = swap.cast()?;
        swap2.SetMaximumFrameLatency(1)?;
        let dxgi: IDXGIDevice = device.cast()?;
        let adapter = dxgi.GetAdapter()?;
        let ad = adapter.GetDesc()?;
        println!(
            "GPU: {}",
            String::from_utf16_lossy(&ad.Description).trim_end_matches('\0')
        );
        let multithread: ID3D11Multithread = context.cast()?;
        let _ = multithread.SetMultithreadProtected(true);
        let video: ID3D11VideoDevice = device.cast()?;
        let vc: ID3D11VideoContext = context.cast()?;
        let mut token = 0;
        let mut manager = None;
        MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
        let manager = manager.context("DXGI device manager missing")?;
        manager.ResetDevice(&device, token)?;
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 6)?;
        let attrs = attrs.unwrap();
        attrs.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &manager)?;
        attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        attrs.SetUINT32(&MF_LOW_LATENCY, 1)?;
        let (reader, mut width, mut height) = if let Some(path) = path {
            let absolute = std::fs::canonicalize(path)?;
            let absolute = absolute.to_string_lossy();
            let absolute = absolute.strip_prefix(r"\\?\").unwrap_or(&absolute);
            let wide: Vec<u16> = absolute.encode_utf16().chain(Some(0)).collect();
            let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attrs)?;
            let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(stream, true)?;
            let mt = MFCreateMediaType()?;
            mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            reader.SetCurrentMediaType(stream, None, &mt)?;
            let mt = reader.GetCurrentMediaType(stream)?;
            let size = mt.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let (width, height) = ((size >> 32) as u32, size as u32);
            (Some(reader), width, height)
        } else {
            (None, 640, 360)
        };
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let mut live_decoder = if live.is_some() {
            Some(crate::decoder::Decoder::new(&manager, width, height)?)
        } else {
            None
        };
        ensure!(width > 0 && height > 0, "invalid video size");
        let (mut enumerator, mut processor) = make_processor(&video, &vc, width, height)?;
        let mut denoise = DENOISE.load(Ordering::Relaxed);
        crate::mouse::register(window.0)?;
        let sampler = Sampler::start_with_config(
            window.0,
            live.as_ref()
                .map(|s| s.input_config.clone())
                .unwrap_or_default(),
        );
        let mut log = input_log.map(std::fs::File::create).transpose()?;
        let start = Instant::now();
        let mut pending_live_sample: Option<(IMFSample, u64)> = None;
        let mut count = 0;
        let mut eof = false;
        let mut first_time = None;
        let mut input_changes = 0;
        'play: loop {
            if crate::cancel::requested() {
                break;
            }
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'play;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let focused = GetForegroundWindow() == window.0;
            let requested_denoise = DENOISE.load(Ordering::Relaxed);
            if requested_denoise != denoise {
                apply_denoise(&enumerator, &vc, &processor, requested_denoise)?;
                denoise = requested_denoise;
            }
            sampler.active.store(focused, Ordering::Relaxed);
            if focused && GetAsyncKeyState(27) < 0 {
                break;
            }
            if seconds > 0 && start.elapsed() >= Duration::from_secs(seconds) {
                break;
            }
            let input = sampler.latest();
            if let Some(source) = &live {
                *source.controls.lock().unwrap() = if focused {
                    crate::feedback::Controls {
                        motion: input.motion,
                        buttons: input.buttons as u64,
                        axes: [input.lx, input.ly, input.rx, input.ry]
                            .map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i32),
                    }
                } else {
                    crate::feedback::Controls {
                        motion: input
                            .motion
                            .map(|m| crate::motion::Pose { active: false, ..m }),
                        ..Default::default()
                    }
                };
            }
            for (elapsed_us, state) in sampler.events() {
                input_changes += 1;
                if let Some(log) = &mut log {
                    writeln!(
                        log,
                        "{}",
                        serde_json::json!({"elapsed_us":elapsed_us,"input":state})
                    )?;
                }
            }
            let title = format!(
                "NS2GameShare {}x{} | frames={} | buttons={:04x} | {} | Esc closes\0",
                width,
                height,
                count,
                input.buttons,
                if focused {
                    "input active"
                } else {
                    "input released"
                }
            );
            let title: Vec<u16> = title.encode_utf16().collect();
            SetWindowTextW(window.0, PCWSTR(title.as_ptr()))?;
            if eof {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            let mut flags = 0;
            let mut time = 0;
            let mut sample = None;
            if let Some(reader) = &reader {
                reader.ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )?;
            } else if let (Some(source), Some(decoder)) = (&mut live, &mut live_decoder) {
                if pending_live_sample.is_none() {
                    match source.receiver.try_recv() {
                        Ok(unit) => {
                            let pts = unit.timestamps.map(|(v, _)| v >> 8);
                            if let Some(decoded) =
                                decoder.push(&unit.payload, unit.sequence, pts)?.pop()
                            {
                                let output_pts = decoded
                                    .GetSampleTime()
                                    .ok()
                                    .map(|v| v.max(0) as u64 * 100)
                                    .unwrap_or(pts.unwrap_or(0));
                                pending_live_sample = Some((decoded, output_pts));
                            }
                            if decoder.width != width || decoder.height != height {
                                width = decoder.width;
                                height = decoder.height;
                                (enumerator, processor) =
                                    make_processor(&video, &vc, width, height)?;
                                println!("映像解像度 {}x{}", width, height);
                            }
                        }
                        Err(TryRecvError::Empty) => {
                            std::thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        Err(TryRecvError::Disconnected) => break 'play,
                    }
                }
                if let Some((_, pts)) = &pending_live_sample
                    && let Some(timeline) = &source.timeline
                {
                    let due = timeline.lock().unwrap().deadline(*pts);
                    if due.is_some_and(|due| due > Instant::now())
                        || (due.is_none() && start.elapsed() < Duration::from_secs(2))
                    {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                }
                sample = pending_live_sample.take().map(|(sample, _)| sample);
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                eof = true;
                if seconds == 0 {
                    println!("EOF（最後のフレームを表示、Esc で終了）");
                }
                continue;
            }
            if let Some(sample) = sample {
                let buffer = sample.GetBufferByIndex(0)?;
                let buffer: IMFDXGIBuffer = buffer
                    .cast()
                    .context("decoder returned CPU memory; D3D11 output is required")?;
                let mut raw = std::ptr::null_mut();
                buffer.GetResource(&ID3D11Texture2D::IID, &mut raw)?;
                let texture = ID3D11Texture2D::from_raw(raw);
                let sub = buffer.GetSubresourceIndex()?;
                let mut td = D3D11_TEXTURE2D_DESC::default();
                texture.GetDesc(&mut td);
                ensure!(
                    td.Format == DXGI_FORMAT_NV12,
                    "unexpected decoder texture format"
                );
                let iv = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    FourCC: 0,
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPIV {
                            MipSlice: sub % td.MipLevels,
                            ArraySlice: sub / td.MipLevels,
                        },
                    },
                };
                let mut input_view = None;
                video.CreateVideoProcessorInputView(
                    &texture,
                    &enumerator,
                    &iv,
                    Some(&mut input_view),
                )?;
                let back: ID3D11Texture2D = swap.GetBuffer(0)?;
                let ov = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                    },
                };
                let mut output_view = None;
                video.CreateVideoProcessorOutputView(
                    &back,
                    &enumerator,
                    &ov,
                    Some(&mut output_view),
                )?;
                let mut surface = D3D11_VIDEO_PROCESSOR_STREAM {
                    Enable: true.into(),
                    pInputSurface: ManuallyDrop::new(input_view),
                    ..Default::default()
                };
                let blit = vc.VideoProcessorBlt(
                    &processor,
                    output_view.as_ref().unwrap(),
                    0,
                    std::slice::from_ref(&surface),
                );
                ManuallyDrop::drop(&mut surface.pInputSurface);
                blit?;
                let base = *first_time.get_or_insert(time);
                let target =
                    Duration::from_nanos(((time - base).max(0) as u64).saturating_mul(100));
                while reader.is_some() && start.elapsed() < target {
                    std::thread::sleep(Duration::from_millis(1));
                }
                swap.Present(0, DXGI_PRESENT(0)).ok()?;
                count += 1;
                if count == 1 {
                    println!(
                        "D3D11 NV12 texture -> GPU video processor -> swapchain: {}x{}（CPU readback なし）",
                        width, height
                    );
                }
            }
        }
        ensure!(count > 0, "no video frames decoded");
        println!(
            "表示フレーム={} 入力状態変化={} live={}",
            count,
            input_changes,
            live.is_some()
        );
    }
    Ok(())
}

unsafe fn make_processor(
    video: &ID3D11VideoDevice,
    vc: &ID3D11VideoContext,
    width: u32,
    height: u32,
) -> Result<(ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor)> {
    unsafe {
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            InputWidth: width,
            InputHeight: height,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            OutputWidth: 1280,
            OutputHeight: 720,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = video.CreateVideoProcessorEnumerator(&content)?;
        let processor = video.CreateVideoProcessor(&enumerator, 0)?;
        let src = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        let ratio = (1280.0 / width as f64).min(720.0 / height as f64);
        let dw = (width as f64 * ratio).round() as i32;
        let dh = (height as f64 * ratio).round() as i32;
        let dst = RECT {
            left: (1280 - dw) / 2,
            top: (720 - dh) / 2,
            right: (1280 + dw) / 2,
            bottom: (720 + dh) / 2,
        };
        vc.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        vc.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src));
        vc.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst));
        vc.VideoProcessorSetOutputTargetRect(
            &processor,
            true,
            Some(&RECT {
                left: 0,
                top: 0,
                right: 1280,
                bottom: 720,
            }),
        );
        vc.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
        apply_denoise(&enumerator, vc, &processor, DENOISE.load(Ordering::Relaxed))?;

        Ok((enumerator, processor))
    }
}
fn apply_denoise(
    enumerator: &ID3D11VideoProcessorEnumerator,
    vc: &ID3D11VideoContext,
    processor: &ID3D11VideoProcessor,
    enabled: bool,
) -> Result<()> {
    unsafe {
        let mut caps = D3D11_VIDEO_PROCESSOR_CAPS::default();
        enumerator.GetVideoProcessorCaps(&mut caps)?;
        if caps.FilterCaps & D3D11_VIDEO_PROCESSOR_FILTER_CAPS_NOISE_REDUCTION.0 as u32 == 0 {
            if enabled {
                println!("GPU noise reduction: unsupported by this video processor");
            }
            return Ok(());
        }
        let range = enumerator
            .GetVideoProcessorFilterRange(D3D11_VIDEO_PROCESSOR_FILTER_NOISE_REDUCTION)?;
        let level = range.Default + ((range.Maximum - range.Default) as f32 * 0.2).round() as i32;
        vc.VideoProcessorSetStreamFilter(
            processor,
            0,
            D3D11_VIDEO_PROCESSOR_FILTER_NOISE_REDUCTION,
            enabled,
            level,
        );
        println!(
            "GPU noise reduction: enabled={} level={} (F9 toggles)",
            enabled, level
        );
    }
    Ok(())
}
