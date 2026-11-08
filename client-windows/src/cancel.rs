use std::sync::atomic::{AtomicBool, Ordering};
static STOP: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static EVENT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn requested() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::{
            Foundation::{HANDLE, WAIT_OBJECT_0},
            System::Threading::WaitForSingleObject,
        };
        let event = EVENT.load(Ordering::Relaxed);
        if event != 0 && unsafe { WaitForSingleObject(HANDLE(event as *mut _), 0) } == WAIT_OBJECT_0
        {
            return true;
        }
    }
    STOP.load(Ordering::Relaxed)
}
#[cfg(windows)]
pub fn open_stop_event(name: &str) -> anyhow::Result<()> {
    use windows::{
        Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenEventW, SYNCHRONIZATION_SYNCHRONIZE},
        },
        core::PCWSTR,
    };
    let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    let event = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr()))? };
    let previous = EVENT.swap(event.0 as usize, Ordering::Relaxed);
    if previous != 0 {
        unsafe {
            let _ = CloseHandle(windows::Win32::Foundation::HANDLE(previous as *mut _));
        }
    }
    Ok(())
}
#[cfg(windows)]
unsafe extern "system" fn handler(event: u32) -> windows::core::BOOL {
    if event == 0 || event == 1 {
        STOP.store(true, Ordering::Relaxed);
        true.into()
    } else {
        false.into()
    }
}
pub fn install() -> anyhow::Result<()> {
    #[cfg(windows)]
    unsafe {
        if !windows::Win32::System::Console::GetConsoleWindow()
            .0
            .is_null()
        {
            windows::Win32::System::Console::SetConsoleCtrlHandler(Some(handler), true)?;
        }
    }
    Ok(())
}
