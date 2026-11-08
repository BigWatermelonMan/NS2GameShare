//! Native bridge for the modern settings window: foreground HID, file dialogs,
//! and the existing event-based playback lifecycle. No rendering on this path.
use super::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    mem::size_of,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::*,
        System::Threading::*,
        UI::{
            Controls::Dialogs::*,
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, PWSTR},
};
static CAPTURE: AtomicBool = AtomicBool::new(false);
static KEY: AtomicU32 = AtomicU32::new(0);
static TESTING: AtomicBool = AtomicBool::new(false);
const SUBCLASS_ID: usize = 0x4e533247;
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
pub fn show_error(message: &str) {
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(wide(message).as_ptr()),
            PCWSTR(wide("NS2GameShare").as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}
unsafe extern "system" fn input_hook(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    unsafe {
        match msg {
            WM_INPUT => {
                let input = windows::Win32::UI::Input::HRAWINPUT(l.0 as *mut _);
                crate::ds4::raw(input);
                crate::mouse::raw(input);
            }
            WM_INPUT_DEVICE_CHANGE => crate::ds4::changed(),
            WM_KILLFOCUS => {
                crate::mouse::release();
                crate::ds4::clear();
                CAPTURE.store(false, Ordering::Relaxed);
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if CAPTURE.load(Ordering::Relaxed) {
                    if l.0 & (1 << 30) == 0 {
                        let _ = KEY.compare_exchange(
                            0,
                            w.0 as u32,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        );
                    }
                    return LRESULT(0);
                }
                if TESTING.load(Ordering::Relaxed) {
                    if w.0 == 27 {
                        crate::mouse::release();
                    } else if w.0 != 0 && w.0 == crate::input::mouse_capture_key() as usize {
                        if l.0 & (1 << 30) == 0 {
                            let _ = crate::mouse::toggle(hwnd);
                        }
                    } else {
                        crate::input::key_down(w.0 as u32);
                    }
                }
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}
pub struct Bridge {
    pub hwnd: HWND,
}
impl Bridge {
    pub fn browse_folder(&self, path: &mut PathBuf) -> Result<bool> {
        use windows::Win32::{System::Com::*, UI::Shell::*};
        unsafe {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            dialog.SetOptions(
                dialog.GetOptions()?
                    | FOS_PICKFOLDERS
                    | FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_NOCHANGEDIR,
            )?;
            if let Err(error) = dialog.Show(Some(self.hwnd)) {
                if error.code().0 as u32 == 0x800704c7 {
                    return Ok(false);
                }
                return Err(error.into());
            }
            let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let text = name.to_string();
            CoTaskMemFree(Some(name.0.cast()));
            *path = PathBuf::from(text?);
            Ok(true)
        }
    }
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let handle = cc
            .window_handle()
            .map_err(|e| anyhow::anyhow!("window handle: {e}"))?;
        let RawWindowHandle::Win32(window) = handle.as_raw() else {
            anyhow::bail!("Windows window required")
        };
        let hwnd = HWND(window.hwnd.get() as *mut _);
        ensure!(
            unsafe { SetWindowSubclass(hwnd, Some(input_hook), SUBCLASS_ID, 0) }.as_bool(),
            "入力フックを作成できません。"
        );
        Ok(Self { hwnd })
    }
    pub fn focused(&self) -> bool {
        unsafe { GetForegroundWindow() == self.hwnd }
    }
    pub fn begin_capture(&self) {
        KEY.store(0, Ordering::Relaxed);
        CAPTURE.store(true, Ordering::Relaxed);
    }
    pub fn take_key(&self) -> Option<u16> {
        let key = KEY.swap(0, Ordering::Relaxed);
        (key != 0).then_some(key as u16)
    }
    pub fn cancel_capture(&self) {
        CAPTURE.store(false, Ordering::Relaxed);
        KEY.store(0, Ordering::Relaxed);
    }
    pub fn start_test(&self, config: crate::input::Config) -> Result<crate::input::Sampler> {
        crate::mouse::register(self.hwnd)?;
        crate::ds4::register(self.hwnd)?;
        let sampler = crate::input::Sampler::start_with_config(self.hwnd, config);
        TESTING.store(true, Ordering::Relaxed);
        Ok(sampler)
    }
    pub fn stop_test(&self) {
        TESTING.store(false, Ordering::Relaxed);
        crate::mouse::release();
        crate::ds4::clear();
    }
    pub fn browse(&self, path: &mut PathBuf, language: i18n::Language) -> Result<bool> {
        unsafe {
            let mut file = vec![0u16; 32768];
            let initial = wide(&path.to_string_lossy());
            if initial.len() < file.len() {
                file[..initial.len()].copy_from_slice(&initial);
            }
            let filter = wide(language.text("すべてのファイル\0*.*\0"));
            let mut options = OPENFILENAMEW {
                lStructSize: size_of::<OPENFILENAMEW>() as u32,
                hwndOwner: self.hwnd,
                lpstrFile: PWSTR(file.as_mut_ptr()),
                nMaxFile: file.len() as u32,
                lpstrFilter: PCWSTR(filter.as_ptr()),
                Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER,
                ..Default::default()
            };
            if GetOpenFileNameW(&mut options).as_bool() {
                let end = file.iter().position(|c| *c == 0).unwrap_or(file.len());
                *path = PathBuf::from(String::from_utf16_lossy(&file[..end]));
                Ok(true)
            } else {
                let code = CommDlgExtendedError().0;
                ensure!(code == 0, "ファイル選択エラー: {code}");
                Ok(false)
            }
        }
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop_test();
        self.cancel_capture();
        unsafe {
            let _ = RemoveWindowSubclass(self.hwnd, Some(input_hook), SUBCLASS_ID);
        }
    }
}
pub fn controllers() -> Result<Vec<crate::dinput::DeviceInfo>> {
    // winit may initialize a different COM apartment on its UI thread.
    std::thread::spawn(crate::dinput::list)
        .join()
        .map_err(|_| anyhow::anyhow!("controller enumeration failed"))?
}
pub fn log_tail(path: &Path) -> String {
    (|| -> std::io::Result<String> {
        let mut f = File::open(path)?;
        let n = f.metadata()?.len();
        f.seek(SeekFrom::Start(n.saturating_sub(4096)))?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })()
    .unwrap_or_default()
}
pub struct Session {
    child: Child,
    event: HANDLE,
    pub log: PathBuf,
    pub stopping: bool,
    pub playing: bool,
}
pub struct Completed {
    pub success: bool,
    pub log: PathBuf,
    pub tail: String,
}
impl Session {
    pub fn start(settings: &Settings, path: &Path) -> Result<Self> {
        settings.validate_connection()?;
        settings.save(path)?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let event_name = format!("Local\\NS2GameShare-{}-{stamp}", std::process::id());
        let event = unsafe { CreateEventW(None, true, false, PCWSTR(wide(&event_name).as_ptr()))? };
        let log = path
            .parent()
            .unwrap_or(Path::new("."))
            .join("logs")
            .join(format!("session-{stamp}.log"));
        let result = (|| -> Result<Child> {
            std::fs::create_dir_all(log.parent().unwrap())?;
            let file = File::create(&log)?;
            Command::new(std::env::current_exe()?)
                .args(settings.play_args(path))
                .args(["--stop-event", &event_name])
                .stdout(Stdio::from(file.try_clone()?))
                .stderr(Stdio::from(file))
                .spawn()
                .context("ゲーム画面を起動できませんでした。")
        })();
        match result {
            Ok(child) => Ok(Self {
                child,
                event,
                log,
                stopping: false,
                playing: false,
            }),
            Err(error) => {
                unsafe {
                    let _ = CloseHandle(event);
                }
                Err(error)
            }
        }
    }
    pub fn stop(&mut self) -> Result<()> {
        unsafe {
            SetEvent(self.event)?;
        }
        self.stopping = true;
        Ok(())
    }
    pub fn poll(&mut self) -> Result<Option<Completed>> {
        if let Some(status) = self.child.try_wait()? {
            return Ok(Some(Completed {
                success: status.success() || self.stopping,
                log: self.log.clone(),
                tail: log_tail(&self.log),
            }));
        }
        if !self.playing {
            let tail = log_tail(&self.log);
            self.playing = tail.contains("H264 unit=") || tail.contains("D3D11 NV12");
        }
        Ok(None)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.event);
            let _ = CloseHandle(self.event);
        }
    }
}
