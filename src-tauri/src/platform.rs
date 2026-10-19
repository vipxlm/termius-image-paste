//! Only platform code may decide that the foreground target is safe to type in.
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static EPOCH: AtomicU64 = AtomicU64::new(0);
static WATCHER_READY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub window: u64,
    pub process: u32,
    pub epoch: u64,
    pub terminal: Option<crate::termius::Connection>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub platform: String,
    pub display_server: String,
    pub automatic_input: bool,
    pub automatic_target_selection: bool,
    pub hotkey_available: bool,
    pub clipboard_available: bool,
    pub tray_available: bool,
    pub description: String,
}

pub fn is_wayland() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some())
}
pub fn capabilities() -> Capabilities {
    let wayland = is_wayland();
    let available = WATCHER_READY.load(Ordering::SeqCst) && !wayland;
    Capabilities {
        platform: if cfg!(windows) { "Windows" } else { "Linux" }.into(),
        display_server: if cfg!(windows) {
            "Win32"
        } else if wayland {
            "Wayland"
        } else {
            "X11"
        }
        .into(),
        automatic_input: available,
        automatic_target_selection: cfg!(windows) && available,
        hotkey_available: available,
        clipboard_available: !wayland,
        tray_available: false,
        description: if wayland {
            "Wayland：请在窗口内粘贴或选择图片或文件上传，再手动取用路径。"
        } else if available {
            "仅在 Termius 位于前台时监听快捷键；切换窗口后停止自动输入。"
        } else {
            "无法可靠检测前台窗口，自动输入已停用；仍可在窗口内上传图片或文件。"
        }
        .into(),
    }
}
pub fn start_watcher() {
    if !is_wayland() {
        native::start_watcher();
    }
}
pub fn capture_target() -> Option<Target> {
    if !WATCHER_READY.load(Ordering::SeqCst) || is_wayland() {
        return None;
    }
    let before = EPOCH.load(Ordering::SeqCst);
    let (window, process) = native::foreground()?;
    if EPOCH.load(Ordering::SeqCst) != before {
        return None;
    }
    Some(Target {
        window,
        process,
        epoch: before,
        terminal: None,
    })
}
pub fn target_unchanged(target: &Target) -> bool {
    let Some(current) = capture_target() else {
        return false;
    };
    if current.window != target.window
        || current.process != target.process
        || current.epoch != target.epoch
    {
        return false;
    }
    target.terminal.as_ref().is_none_or(|terminal| {
        crate::termius::capture(target.process)
            .as_ref()
            .is_ok_and(|current| current == terminal)
    })
}
pub fn type_path(target: &Target, path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > 16384 || path.chars().any(char::is_control) {
        return Err("待输入路径无效，未发送任何按键。".into());
    }
    if !target_unchanged(target) {
        return Err("上传已完成，但输入窗口发生变化，未填入路径。".into());
    }
    native::type_text(target, path)
}
pub fn process_executable(process: u32) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        native::process_executable(process)
    }
    #[cfg(not(windows))]
    {
        let _ = process;
        None
    }
}
pub fn modifiers_down() -> bool {
    native::modifiers_down()
}

/// Explorer exposes copied files as CF_HDROP. Read only; never clear the clipboard.
/// Other desktops use in-window file paste, selection or drag-and-drop.
pub fn clipboard_file() -> crate::core::AppResult<Option<std::path::PathBuf>> {
    #[cfg(windows)]
    {
        use crate::core::AppError;
        use std::{
            ffi::{c_void, OsString},
            os::windows::ffi::OsStringExt,
            ptr,
        };
        #[link(name = "user32")]
        extern "system" {
            fn OpenClipboard(owner: *mut c_void) -> i32;
            fn CloseClipboard() -> i32;
            fn IsClipboardFormatAvailable(format: u32) -> i32;
            fn GetClipboardData(format: u32) -> *mut c_void;
        }
        #[link(name = "shell32")]
        extern "system" {
            fn DragQueryFileW(drop: *mut c_void, index: u32, buffer: *mut u16, length: u32) -> u32;
        }
        struct ClipboardGuard;
        impl Drop for ClipboardGuard {
            fn drop(&mut self) {
                unsafe {
                    CloseClipboard();
                }
            }
        }
        unsafe {
            if OpenClipboard(ptr::null_mut()) == 0 {
                return Err(AppError::new(
                    "clipboard_unavailable",
                    "剪贴板正被其他程序使用，请稍后重试。",
                ));
            }
            let _guard = ClipboardGuard;
            if IsClipboardFormatAvailable(15) == 0 {
                return Ok(None);
            }
            let drop = GetClipboardData(15);
            if drop.is_null() {
                return Err(AppError::new(
                    "clipboard_unavailable",
                    "无法读取剪贴板文件，请重新复制。",
                ));
            }
            if DragQueryFileW(drop, u32::MAX, ptr::null_mut(), 0) != 1 {
                return Err(AppError::new(
                    "multiple_files",
                    "请每次只复制一个文件，暂不支持批量上传。",
                ));
            }
            let length = DragQueryFileW(drop, 0, ptr::null_mut(), 0);
            if length == 0 || length > 32767 {
                return Err(AppError::new("invalid_file", "剪贴板文件路径无效。"));
            }
            let mut buffer = vec![0; length as usize + 1];
            if DragQueryFileW(drop, 0, buffer.as_mut_ptr(), length + 1) != length {
                return Err(AppError::new("invalid_file", "读取剪贴板文件路径失败。"));
            }
            return Ok(Some(OsString::from_wide(&buffer[..length as usize]).into()));
        }
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{ffi::c_void, mem::size_of, ptr};
    type Handle = *mut c_void;
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    struct Message {
        window: Handle,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        point: Point,
        private: u32,
    }
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KeyboardInput {
        vk: u16,
        scan: u16,
        flags: u32,
        time: u32,
        extra: usize,
    }
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct MouseInput {
        dx: i32,
        dy: i32,
        data: u32,
        flags: u32,
        time: u32,
        extra: usize,
    }
    #[repr(C)]
    union InputData {
        keyboard: KeyboardInput,
        mouse: MouseInput,
    }
    #[repr(C)]
    struct Input {
        kind: u32,
        data: InputData,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetForegroundWindow() -> Handle;
        fn GetWindowThreadProcessId(window: Handle, pid: *mut u32) -> u32;
        fn GetAsyncKeyState(key: i32) -> i16;
        fn SendInput(count: u32, inputs: *const Input, size: i32) -> u32;
        fn SetWinEventHook(
            min: u32,
            max: u32,
            module: Handle,
            callback: unsafe extern "system" fn(Handle, u32, Handle, i32, i32, u32, u32),
            pid: u32,
            thread: u32,
            flags: u32,
        ) -> Handle;
        fn GetMessageW(message: *mut Message, window: Handle, min: u32, max: u32) -> i32;
        fn UnhookWinEvent(hook: Handle) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    unsafe extern "system" fn changed(
        _: Handle,
        _: u32,
        _: Handle,
        _: i32,
        _: i32,
        _: u32,
        _: u32,
    ) {
        EPOCH.fetch_add(1, Ordering::SeqCst);
    }
    pub fn start_watcher() {
        std::thread::spawn(|| unsafe {
            let hook = SetWinEventHook(3, 3, ptr::null_mut(), changed, 0, 0, 0);
            if hook.is_null() {
                return;
            }
            WATCHER_READY.store(true, Ordering::SeqCst);
            let mut msg: Message = std::mem::zeroed();
            while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {}
            WATCHER_READY.store(false, Ordering::SeqCst);
            UnhookWinEvent(hook);
        });
    }
    pub fn process_executable(pid: u32) -> Option<std::path::PathBuf> {
        unsafe {
            let process = OpenProcess(0x1000, 0, pid);
            if process.is_null() {
                return None;
            }
            let mut name = [0u16; 32768];
            let mut length = name.len() as u32;
            let ok = QueryFullProcessImageNameW(process, 0, name.as_mut_ptr(), &mut length);
            CloseHandle(process);
            if ok == 0 {
                return None;
            }
            Some(std::path::PathBuf::from(String::from_utf16_lossy(
                &name[..length as usize],
            )))
        }
    }
    pub fn foreground() -> Option<(u64, u32)> {
        unsafe {
            let window = GetForegroundWindow();
            if window.is_null() {
                return None;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            let executable = process_executable(pid)?;
            if !executable
                .file_name()?
                .to_str()?
                .eq_ignore_ascii_case("Termius.exe")
            {
                return None;
            }
            Some((window as u64, pid))
        }
    }
    pub fn modifiers_down() -> bool {
        [0x10, 0x11, 0x12, 0x5b, 0x5c]
            .iter()
            .any(|key| unsafe { GetAsyncKeyState(*key) < 0 })
    }
    pub fn type_text(target: &Target, text: &str) -> Result<(), String> {
        if modifiers_down() {
            return Err("快捷键尚未松开，路径未自动输入。".into());
        }
        let mut events = Vec::with_capacity(text.len() * 2);
        for unit in text.encode_utf16() {
            for flags in [4, 6] {
                events.push(Input {
                    kind: 1,
                    data: InputData {
                        keyboard: KeyboardInput {
                            vk: 0,
                            scan: unit,
                            flags,
                            time: 0,
                            extra: 0,
                        },
                    },
                });
            }
        }
        if !target_unchanged(target) {
            return Err("目标窗口发生变化，路径未自动输入。".into());
        }
        let sent = unsafe {
            SendInput(
                events.len() as u32,
                events.as_ptr(),
                size_of::<Input>() as i32,
            )
        };
        if sent as usize != events.len() {
            return Err(
                "系统阻止了输入，请确认 Termius 没有以更高权限运行；可手动取用路径。".into(),
            );
        }
        Ok(())
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn win_input_layout() {
            assert_eq!(
                size_of::<Input>(),
                if size_of::<usize>() == 8 { 40 } else { 28 }
            )
        }
    }
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use std::{
        sync::{Mutex, OnceLock},
        time::Duration,
    };
    use x11rb::{
        connection::Connection,
        protocol::{
            xkb::{self, ConnectionExt as _, GetMapReply, GetStateReply, KeySymMap, KeyType},
            xproto::{
                self, Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask,
                ModMask, Window,
            },
            xtest::ConnectionExt as _,
            Event,
        },
        rust_connection::RustConnection,
        wrapper::ConnectionExt as _,
    };

    // A single connection and lock let foreground reads drain the very same event
    // queue used by the watcher. A rapid away-and-back cannot hide in another queue.
    struct Desktop {
        connection: RustConnection,
        root: Window,
        active: Atom,
        pid: Atom,
    }
    static DESKTOP: OnceLock<Mutex<Desktop>> = OnceLock::new();
    static STARTED: AtomicBool = AtomicBool::new(false);
    const CORE_KEYBOARD: u16 = 0x0100;

    impl Desktop {
        fn open() -> Option<Self> {
            let (connection, screen) = x11rb::connect(None).ok()?;
            let root = connection.setup().roots.get(screen)?.root;
            let active = connection
                .intern_atom(false, b"_NET_ACTIVE_WINDOW")
                .ok()?
                .reply()
                .ok()?
                .atom;
            let pid = connection
                .intern_atom(false, b"_NET_WM_PID")
                .ok()?
                .reply()
                .ok()?
                .atom;
            connection.xtest_get_version(2, 2).ok()?.reply().ok()?;
            if !connection
                .xkb_use_extension(1, 0)
                .ok()?
                .reply()
                .ok()?
                .supported
            {
                return None;
            }
            let desktop = Self {
                connection,
                root,
                active,
                pid,
            };
            desktop.property(root, active, AtomEnum::WINDOW.into())?;
            desktop
                .connection
                .change_window_attributes(
                    root,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                )
                .ok()?
                .check()
                .ok()?;
            Some(desktop)
        }
        fn property(&self, window: Window, atom: Atom, expected: Atom) -> Option<u64> {
            // A closed window is an ordinary checked reply error, never an Xlib
            // process-global error handler (which is shared with GTK/WebKit).
            let reply = self
                .connection
                .get_property(false, window, atom, expected, 0, 1)
                .ok()?
                .reply()
                .ok()?;
            if reply.type_ != expected || reply.format != 32 || reply.value_len != 1 {
                return None;
            }
            let value = reply.value32()?.next().map(u64::from);
            value
        }
        fn drain_events(&self) -> Result<(), String> {
            self.connection
                .sync()
                .map_err(|_| "X11 连接已中断，自动输入已停用。")?;
            loop {
                match self
                    .connection
                    .poll_for_event()
                    .map_err(|_| "X11 事件读取失败，自动输入已停用。")?
                {
                    Some(Event::PropertyNotify(event))
                        if event.window == self.root && event.atom == self.active =>
                    {
                        EPOCH.fetch_add(1, Ordering::SeqCst);
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            Ok(())
        }
        fn foreground(&self) -> Option<(u64, u32)> {
            let window = self.property(self.root, self.active, AtomEnum::WINDOW.into())?;
            if window == 0 {
                return None;
            }
            let pid = self.property(window as _, self.pid, AtomEnum::CARDINAL.into())?;
            if pid == 0 || pid > u32::MAX as u64 {
                return None;
            }
            let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
            let name = executable.file_name()?.to_str()?.to_ascii_lowercase();
            if !["termius", "termius-app"].contains(&name.as_str()) {
                return None;
            }
            Some((window, pid as u32))
        }
        fn parent_of(&self, window: Window) -> Option<Window> {
            self.connection
                .query_tree(window)
                .ok()?
                .reply()
                .ok()
                .map(|tree| tree.parent)
        }
        fn focus_belongs_to(&self, window: u64) -> bool {
            let Some(mut focus) = self
                .connection
                .get_input_focus()
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.focus)
            else {
                return false;
            };
            for _ in 0..32 {
                if u64::from(focus) == window {
                    return true;
                }
                if focus <= 1 || focus == self.root {
                    return false;
                }
                let Some(parent) = self.parent_of(focus) else {
                    return false;
                };
                if parent == focus {
                    return false;
                }
                focus = parent;
            }
            false
        }
        fn keyboard_map(&self) -> Result<GetMapReply, String> {
            self.connection
                .xkb_get_map(
                    CORE_KEYBOARD,
                    xkb::MapPart::KEY_TYPES | xkb::MapPart::KEY_SYMS | xkb::MapPart::MODIFIER_MAP,
                    0u16.into(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0u16.into(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                )
                .map_err(|_| "无法读取 X11 键盘布局。")?
                .reply()
                .map_err(|_| "无法读取 X11 键盘布局。".into())
        }
        fn keyboard_state(&self, map: &GetMapReply) -> Result<GetStateReply, String> {
            let state = self
                .connection
                .xkb_get_state(CORE_KEYBOARD)
                .map_err(|_| "无法确认 X11 键盘状态。")?
                .reply()
                .map_err(|_| "无法确认 X11 键盘状态。")?;
            let mut num_lock_mask = 0;
            for modifier in map
                .map
                .modmap_rtrn
                .as_deref()
                .ok_or("X11 修饰键映射为空。")?
            {
                if symbol_for(map, modifier.keycode, 0) == Some(0xff7f) {
                    num_lock_mask |= u16::from(modifier.mods);
                }
            }
            // Unknown locks, latched/held modifiers and non-default groups fail
            // closed. Only NumLock identified from this fresh map is tolerated.
            if !safe_keyboard_state(&state, num_lock_mask) {
                return Err(
                    "当前键盘有修饰键、CapsLock 或其他布局组状态，请松开按键或手动取用路径。"
                        .into(),
                );
            }
            Ok(state)
        }
        fn encode(
            &self,
            text: &str,
            map: &GetMapReply,
            state: &GetStateReply,
        ) -> Result<(Vec<(u32, bool)>, u32), String> {
            let mut candidates = Vec::new();
            let mut shift = 0;
            let modifiers = map
                .map
                .modmap_rtrn
                .as_deref()
                .ok_or("无法核对 Shift 键映射。")?;
            for code in map.min_key_code..=map.max_key_code {
                for shifted in [false, true] {
                    let mods = u16::from(state.mods)
                        | if shifted {
                            u16::from(ModMask::SHIFT)
                        } else {
                            0
                        };
                    if let Some(symbol) = symbol_for(map, code, mods) {
                        if !shifted
                            && matches!(symbol, 0xffe1 | 0xffe2)
                            && modifiers
                                .iter()
                                .any(|m| m.keycode == code && m.mods == ModMask::SHIFT)
                        {
                            shift = u32::from(code);
                        }
                        candidates.push((u64::from(symbol), u32::from(code), shifted));
                    }
                }
            }
            let strokes = encode_characters(text, &candidates)?;
            if strokes.iter().any(|(_, shifted)| *shifted) && shift == 0 {
                return Err("当前键盘缺少可用 Shift 键，请手动取用路径。".into());
            }
            Ok((strokes, shift))
        }
    }

    fn symbol_for(map: &GetMapReply, code: u8, mods: u16) -> Option<u32> {
        let key = map
            .map
            .syms_rtrn
            .as_ref()?
            .get(usize::from(code.checked_sub(map.first_key_sym)?))?;
        let kind = map
            .map
            .types_rtrn
            .as_ref()?
            .get(usize::from(key.kt_index[0].checked_sub(map.first_type)?))?;
        group_zero_symbol(key, kind, mods)
    }
    fn group_zero_symbol(key: &KeySymMap, kind: &KeyType, mods: u16) -> Option<u32> {
        // Automatic input only accepts group zero. Select the active level using
        // the server's resolved real modifier masks; no layout or clipboard edits.
        if key.group_info & 0x0f == 0 || key.width == 0 {
            return None;
        }
        let level = kind
            .map
            .iter()
            .find(|entry| {
                entry.active && mods & u16::from(kind.mods_mask) == u16::from(entry.mods_mask)
            })
            .map_or(0, |entry| entry.level);
        if level >= kind.num_levels || level >= key.width {
            return None;
        }
        key.syms
            .get(usize::from(level))
            .copied()
            .filter(|symbol| *symbol != 0)
    }
    fn safe_keyboard_state(state: &GetStateReply, num_lock_mask: u16) -> bool {
        // Even an unusual NumLock mapping must not allow Shift/Lock/Control:
        // those core bits can still change terminal text independently of keysyms.
        let num_lock_mask = num_lock_mask & 0xf8;
        u8::from(state.group) == 0
            && state.base_group == 0
            && state.latched_group == 0
            && u8::from(state.locked_group) == 0
            && u16::from(state.base_mods) == 0
            && u16::from(state.latched_mods) == 0
            && u16::from(state.locked_mods) & !num_lock_mask == 0
            && u16::from(state.mods) & !num_lock_mask == 0
    }
    fn encode_characters(
        text: &str,
        candidates: &[(u64, u32, bool)],
    ) -> Result<Vec<(u32, bool)>, String> {
        text.chars()
            .map(|ch| {
                let symbol = if ch as u32 <= 0xff {
                    ch as u64
                } else {
                    0x01000000 | ch as u64
                };
                candidates
                    .iter()
                    .find(|(candidate, _, _)| {
                        *candidate == symbol || (ch == '€' && *candidate == 0x20ac)
                    })
                    .map(|(_, code, shifted)| (*code, *shifted))
                    .ok_or_else(|| "当前 X11 键盘布局无法完整输入此路径，请手动取用路径。".into())
            })
            .collect()
    }
    struct ServerGrab<'a>(&'a Desktop);
    impl<'a> ServerGrab<'a> {
        fn new(desktop: &'a Desktop) -> Result<Self, String> {
            desktop
                .connection
                .grab_server()
                .map_err(|_| "无法锁定 X11 输入目标。")?
                .check()
                .map_err(|_| "无法锁定 X11 输入目标。")?;
            Ok(Self(desktop))
        }
    }
    impl Drop for ServerGrab<'_> {
        fn drop(&mut self) {
            if let Ok(cookie) = self.0.connection.ungrab_server() {
                let _ = cookie.check();
            }
        }
    }

    pub fn start_watcher() {
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(|| {
            let Some(desktop) = Desktop::open() else {
                return;
            };
            if DESKTOP.set(Mutex::new(desktop)).is_err() {
                return;
            }
            WATCHER_READY.store(true, Ordering::SeqCst);
            loop {
                let Some(connection) = DESKTOP.get() else {
                    break;
                };
                let Ok(desktop) = connection.lock() else {
                    break;
                };
                if desktop.drain_events().is_err() {
                    break;
                }
                drop(desktop);
                std::thread::sleep(Duration::from_millis(20));
            }
            WATCHER_READY.store(false, Ordering::SeqCst);
        });
    }
    pub fn foreground() -> Option<(u64, u32)> {
        let desktop = DESKTOP.get()?.lock().ok()?;
        let _grab = ServerGrab::new(&desktop).ok()?;
        desktop.drain_events().ok()?;
        desktop.foreground()
    }
    pub fn modifiers_down() -> bool {
        DESKTOP
            .get()
            .and_then(|connection| connection.lock().ok())
            .is_none_or(|desktop| {
                let Ok(_grab) = ServerGrab::new(&desktop) else {
                    return true;
                };
                desktop
                    .keyboard_map()
                    .and_then(|map| desktop.keyboard_state(&map))
                    .is_err()
            })
    }
    pub fn type_text(target: &Target, text: &str) -> Result<(), String> {
        let desktop = DESKTOP
            .get()
            .and_then(|connection| connection.lock().ok())
            .ok_or("X11 前台监测不可用，请手动取用路径。")?;
        let _grab = ServerGrab::new(&desktop)?;
        desktop.drain_events()?;
        if desktop.foreground() != Some((target.window, target.process))
            || EPOCH.load(Ordering::SeqCst) != target.epoch
            || !desktop.focus_belongs_to(target.window)
        {
            return Err("目标窗口发生变化，路径未自动输入。".into());
        }
        let map = desktop.keyboard_map()?;
        let keyboard = desktop.keyboard_state(&map)?;
        let (strokes, shift) = desktop.encode(text, &map, &keyboard)?;
        // All characters have been resolved before the first event is sent.
        let mut complete = true;
        let send = |code: u32, press: bool| {
            desktop
                .connection
                .xtest_fake_input(
                    if press {
                        xproto::KEY_PRESS_EVENT
                    } else {
                        xproto::KEY_RELEASE_EVENT
                    },
                    code as u8,
                    0,
                    desktop.root,
                    0,
                    0,
                    0,
                )
                .ok()
                .is_some_and(|cookie| cookie.check().is_ok())
        };
        for (code, shifted) in strokes {
            if shifted {
                complete &= send(shift, true);
            }
            complete &= send(code, true);
            complete &= send(code, false);
            if shifted {
                complete &= send(shift, false);
            }
            if !complete {
                break;
            }
        }
        if !complete {
            return Err("系统未能完整输入路径，请检查输入框并手动取用路径。".into());
        }
        Ok(())
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn full_path_must_be_encodable() {
            let candidates = vec![(b'a' as u64, 38, false), (b'A' as u64, 38, true)];
            assert_eq!(
                encode_characters("aA", &candidates).unwrap(),
                vec![(38, false), (38, true)]
            );
            assert!(encode_characters("a图", &candidates).is_err());
        }
        #[test]
        fn caps_and_group_states_fail_closed_but_numlock_is_identified() {
            let mut state = GetStateReply::default();
            assert!(safe_keyboard_state(&state, 16));
            state.mods = 16u16.into();
            state.locked_mods = 16u16.into();
            assert!(safe_keyboard_state(&state, 16));
            state.mods |= ModMask::LOCK;
            state.locked_mods |= ModMask::LOCK;
            assert!(!safe_keyboard_state(&state, 16));
            assert!(!safe_keyboard_state(&state, 18));
            state = GetStateReply::default();
            state.group = 1u8.into();
            assert!(!safe_keyboard_state(&state, 16));
            state.group = 0u8.into();
            state.latched_mods = ModMask::SHIFT;
            assert!(!safe_keyboard_state(&state, 16));
        }
        #[test]
        fn xkb_levels_use_resolved_masks_and_reject_missing_symbols() {
            let mut key = KeySymMap {
                kt_index: [0; 4],
                group_info: 1,
                width: 2,
                syms: vec![b'a' as u32, b'A' as u32],
            };
            let kind = KeyType {
                mods_mask: ModMask::SHIFT,
                num_levels: 2,
                map: vec![xkb::KTMapEntry {
                    active: true,
                    mods_mask: ModMask::SHIFT,
                    level: 1,
                    ..Default::default()
                }],
                ..Default::default()
            };
            assert_eq!(group_zero_symbol(&key, &kind, 0), Some(b'a' as u32));
            assert_eq!(group_zero_symbol(&key, &kind, 1), Some(b'A' as u32));
            assert_eq!(group_zero_symbol(&key, &kind, 16), Some(b'a' as u32));
            key.syms.truncate(1);
            assert_eq!(group_zero_symbol(&key, &kind, 1), None);
            key.group_info = 0;
            assert_eq!(group_zero_symbol(&key, &kind, 0), None);
        }
        #[test]
        fn stale_window_queries_fail_closed() {
            let Ok((connection, screen)) = x11rb::connect(None) else {
                eprintln!("X11 fixture unavailable; stale-window runtime check skipped");
                return;
            };
            let root = connection.setup().roots[screen].root;
            let desktop = Desktop {
                connection,
                root,
                active: AtomEnum::WINDOW.into(),
                pid: AtomEnum::CARDINAL.into(),
            };
            let window = desktop.connection.generate_id().unwrap();
            desktop
                .connection
                .create_window(
                    0,
                    window,
                    root,
                    0,
                    0,
                    1,
                    1,
                    0,
                    xproto::WindowClass::INPUT_OUTPUT,
                    0,
                    &xproto::CreateWindowAux::new(),
                )
                .unwrap()
                .check()
                .unwrap();
            desktop
                .connection
                .destroy_window(window)
                .unwrap()
                .check()
                .unwrap();
            assert_eq!(
                desktop.property(window, desktop.pid, AtomEnum::CARDINAL.into()),
                None
            );
            assert_eq!(desktop.parent_of(window), None);
            desktop.connection.sync().unwrap();
            if desktop
                .connection
                .xkb_use_extension(1, 0)
                .unwrap()
                .reply()
                .unwrap()
                .supported
            {
                let map = desktop.keyboard_map().unwrap();
                let state = desktop
                    .connection
                    .xkb_get_state(CORE_KEYBOARD)
                    .unwrap()
                    .reply()
                    .unwrap();
                if safe_keyboard_state(&state, 0) {
                    let encoded = desktop.encode("test/' pathA1.png", &map, &state);
                    eprintln!(
                        "X11 current layout encodes common ASCII: {}",
                        encoded.is_ok()
                    );
                }
            }
            eprintln!(
                "X11 destroyed-window property/tree errors handled without a global Xlib handler"
            );
        }
    }
}
#[cfg(not(any(windows, target_os = "linux")))]
mod native {
    use super::*;
    pub fn start_watcher() {}
    pub fn foreground() -> Option<(u64, u32)> {
        None
    }
    pub fn modifiers_down() -> bool {
        true
    }
    pub fn type_text(_: &Target, _: &str) -> Result<(), String> {
        Err("当前系统不支持自动输入。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn epoch_and_process_are_part_of_target() {
        let a = Target {
            window: 1,
            process: 3,
            epoch: 4,
            terminal: None,
        };
        let mut b = a.clone();
        b.epoch += 1;
        assert_ne!(a, b);
        b = a.clone();
        b.process += 1;
        assert_ne!(a, b)
    }
}
