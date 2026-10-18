//! Reads one focused Termius connection using the OS HTTP/WebSocket client.
use crate::core::{AppError, AppResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub terminal_id: String,
    pub connection_id: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub target_id: String,
}
impl Connection {
    pub fn endpoint(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!("{}@{}:{}", self.username, host, self.port)
    }
}
fn failure(message: &str) -> AppError {
    AppError::new("termius_connection", message)
}
fn observation(value: serde_json::Value, target_id: &str) -> AppResult<Option<Connection>> {
    match value["status"].as_str() {
        Some("not-focused") => return Ok(None),
        Some("connection-found") if value["broadcastChecked"] == true => {}
        Some("broadcast-active") => {
            return Err(failure("Termius 已开启广播输入，请关闭广播后再贴图。"))
        }
        Some("terminal-not-focused") => {
            return Err(failure("请先点击 Termius 的服务器终端输入区域。"))
        }
        Some("shell-not-ready") => return Err(failure("当前终端尚未连接，请先完成 SSH 登录。")),
        _ => return Err(failure("无法可靠识别当前 Termius 连接，已停止自动上传。")),
    }
    let mut connection: Connection =
        serde_json::from_value(value).map_err(|_| failure("Termius 连接字段不完整。"))?;
    let valid = |s: &str| !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control);
    if ![
        &connection.host,
        &connection.username,
        &connection.terminal_id,
        &connection.connection_id,
    ]
    .into_iter()
    .all(|s| valid(s))
        || connection.port == 0
        || !["ssh", "mosh"].contains(&connection.protocol.as_str())
    {
        return Err(failure("Termius 连接字段无效。"));
    }
    connection.target_id = target_id.into();
    Ok(Some(connection))
}
pub fn capture(process: u32) -> AppResult<Connection> {
    #[cfg(windows)]
    {
        native::capture(process)
    }
    #[cfg(not(windows))]
    {
        let _ = process;
        Err(failure("当前系统尚不支持读取 Termius 真实连接。"))
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        ffi::c_void,
        fs::File,
        io::Read,
        path::PathBuf,
        ptr,
        sync::Mutex,
        time::{Duration, Instant, SystemTime},
    };
    type Handle = *mut c_void;
    #[link(name = "winhttp")]
    unsafe extern "system" {
        fn WinHttpOpen(
            agent: *const u16,
            access: u32,
            proxy: *const u16,
            bypass: *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpConnect(session: Handle, server: *const u16, port: u16, reserved: u32) -> Handle;
        fn WinHttpOpenRequest(
            connection: Handle,
            verb: *const u16,
            path: *const u16,
            version: *const u16,
            referer: *const u16,
            accept: *const *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpSetTimeouts(
            handle: Handle,
            resolve: i32,
            connect: i32,
            send: i32,
            receive: i32,
        ) -> i32;
        fn WinHttpSetOption(handle: Handle, option: u32, buffer: *const c_void, length: u32)
            -> i32;
        fn WinHttpSendRequest(
            handle: Handle,
            headers: *const u16,
            header_length: u32,
            data: *const c_void,
            data_length: u32,
            total: u32,
            context: usize,
        ) -> i32;
        fn WinHttpReceiveResponse(handle: Handle, reserved: *mut c_void) -> i32;
        fn WinHttpQueryHeaders(
            handle: Handle,
            level: u32,
            name: *const u16,
            buffer: *mut c_void,
            size: *mut u32,
            index: *mut u32,
        ) -> i32;
        fn WinHttpReadData(handle: Handle, buffer: *mut c_void, length: u32, read: *mut u32)
            -> i32;
        fn WinHttpCloseHandle(handle: Handle) -> i32;
        fn WinHttpWebSocketCompleteUpgrade(handle: Handle, context: usize) -> Handle;
        fn WinHttpWebSocketSend(
            handle: Handle,
            kind: u32,
            buffer: *const c_void,
            length: u32,
        ) -> u32;
        fn WinHttpWebSocketClose(
            handle: Handle,
            status: u16,
            reason: *const c_void,
            length: u32,
        ) -> u32;
        fn WinHttpWebSocketReceive(
            handle: Handle,
            buffer: *mut c_void,
            length: u32,
            read: *mut u32,
            kind: *mut u32,
        ) -> u32;
    }
    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn GetExtendedTcpTable(
            table: *mut c_void,
            size: *mut u32,
            sorted: i32,
            family: u32,
            class: u32,
            reserved: u32,
        ) -> u32;
    }
    struct Internet(Handle);
    impl Drop for Internet {
        fn drop(&mut self) {
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
    fn own(handle: Handle) -> AppResult<Internet> {
        if handle.is_null() {
            Err(failure("无法建立本机 Termius 读取通道。"))
        } else {
            Ok(Internet(handle))
        }
    }
    fn check(ok: i32) -> AppResult<()> {
        if ok == 0 {
            Err(failure(&format!(
                "本机 Termius 连接读取失败或超时（系统错误 {}）。",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            )))
        } else {
            Ok(())
        }
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    struct Http {
        connection: Internet,
        session: Internet,
    }
    impl Http {
        fn open(port: u16) -> AppResult<Self> {
            unsafe {
                let session = own(WinHttpOpen(
                    wide("TermiusImagePaste/2").as_ptr(),
                    1,
                    ptr::null(),
                    ptr::null(),
                    0,
                ))?;
                check(WinHttpSetTimeouts(session.0, 250, 250, 500, 500))?;
                let connection = own(WinHttpConnect(
                    session.0,
                    wide("127.0.0.1").as_ptr(),
                    port,
                    0,
                ))?;
                Ok(Self {
                    session,
                    connection,
                })
            }
        }
        fn request(&self, path: &str, websocket: bool) -> AppResult<Internet> {
            let _ = &self.session;
            unsafe {
                let request = own(WinHttpOpenRequest(
                    self.connection.0,
                    wide("GET").as_ptr(),
                    wide(path).as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    0,
                ))?;
                let never: u32 = 0;
                check(WinHttpSetOption(request.0, 88, &never as *const _ as _, 4))?;
                if websocket {
                    check(WinHttpSetOption(request.0, 114, ptr::null(), 0))?;
                }
                check(WinHttpSendRequest(
                    request.0,
                    ptr::null(),
                    0,
                    ptr::null(),
                    0,
                    0,
                    0,
                ))?;
                check(WinHttpReceiveResponse(request.0, ptr::null_mut()))?;
                let mut status: u32 = 0;
                let mut length = 4;
                check(WinHttpQueryHeaders(
                    request.0,
                    19 | 0x20000000,
                    ptr::null(),
                    &mut status as *mut _ as _,
                    &mut length,
                    ptr::null_mut(),
                ))?;
                if status != if websocket { 101 } else { 200 } {
                    return Err(failure("Termius 本机接口返回了不支持的响应。"));
                }
                Ok(request)
            }
        }
        fn json(&self, path: &str) -> AppResult<serde_json::Value> {
            let request = self.request(path, false)?;
            let mut result = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                let mut count = 0;
                unsafe {
                    check(WinHttpReadData(
                        request.0,
                        buffer.as_mut_ptr() as _,
                        buffer.len() as u32,
                        &mut count,
                    ))?;
                }
                if count == 0 {
                    break;
                }
                result.extend_from_slice(&buffer[..count as usize]);
                if result.len() > 1024 * 1024 {
                    return Err(failure("Termius 本机响应超过读取上限。"));
                }
            }
            serde_json::from_slice(&result).map_err(|_| failure("Termius 本机接口未返回有效数据。"))
        }
        fn probe(&self, path: &str) -> AppResult<serde_json::Value> {
            let request = self.request(path, true)?;
            let websocket = own(unsafe { WinHttpWebSocketCompleteUpgrade(request.0, 0) })?;
            let timeout: u32 = 250;
            unsafe {
                check(WinHttpSetOption(
                    websocket.0,
                    115,
                    &timeout as *const _ as _,
                    4,
                ))?;
            }
            let expression = format!("({})()", include_str!("termius-connection-probe.js"));
            let message = serde_json::json!({"id":1,"method":"Runtime.evaluate","params":{"expression":expression,"returnByValue":true,"silent":true,"throwOnSideEffect":true}}).to_string();
            if unsafe {
                WinHttpWebSocketSend(websocket.0, 2, message.as_ptr() as _, message.len() as u32)
            } != 0
            {
                return Err(failure("无法发送 Termius 只读请求。"));
            }
            let (finished, wait) = std::sync::mpsc::channel();
            let socket = websocket.0 as usize;
            std::thread::scope(|scope| {
                // A WebSocket handle does not accept WINHTTP_OPTION_RECEIVE_TIMEOUT.
                // Close the WebSocket protocol on timeout to cancel the pending read;
                // keep its native handle alive until this scoped worker has finished.
                scope.spawn(move || {
                    if matches!(
                        wait.recv_timeout(Duration::from_secs(2)),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    ) {
                        unsafe {
                            WinHttpWebSocketClose(socket as Handle, 1000, ptr::null(), 0);
                        }
                    }
                });
                let result = (|| {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    let mut result = Vec::new();
                    let mut buffer = [0u8; 8192];
                    while Instant::now() < deadline {
                        let mut count = 0;
                        let mut kind = 0;
                        if unsafe {
                            WinHttpWebSocketReceive(
                                websocket.0,
                                buffer.as_mut_ptr() as _,
                                buffer.len() as u32,
                                &mut count,
                                &mut kind,
                            )
                        } != 0
                            || ![2, 3].contains(&kind)
                        {
                            return Err(failure("Termius 只读通道已关闭或超时。"));
                        }
                        result.extend_from_slice(&buffer[..count as usize]);
                        if result.len() > 1024 * 1024 {
                            return Err(failure("Termius 连接响应超过读取上限。"));
                        }
                        if kind == 3 {
                            continue;
                        }
                        let response: serde_json::Value = serde_json::from_slice(&result)
                            .map_err(|_| failure("Termius 连接响应无效。"))?;
                        result.clear();
                        if response["id"] != 1 {
                            continue;
                        }
                        if !response["error"].is_null()
                            || !response["result"]["exceptionDetails"].is_null()
                        {
                            return Err(failure(
                                "Termius 版本的只读连接读取失败，已停止自动上传。",
                            ));
                        }
                        return Ok(response["result"]["result"]["value"].clone());
                    }
                    Err(failure("读取 Termius 连接超时。"))
                })();
                let _ = finished.send(());
                result
            })
        }
    }
    fn ports(process: u32) -> AppResult<Vec<u16>> {
        let mut bytes = 0;
        if unsafe { GetExtendedTcpTable(ptr::null_mut(), &mut bytes, 0, 2, 3, 0) } != 122
            || !(4..=4 * 1024 * 1024).contains(&bytes)
        {
            return Err(failure("无法核验 Termius 本机端口归属。"));
        }
        let mut table = vec![0u32; (bytes as usize).div_ceil(4)];
        if unsafe { GetExtendedTcpTable(table.as_mut_ptr() as _, &mut bytes, 0, 2, 3, 0) } != 0 {
            return Err(failure("Termius 本机端口状态发生变化。"));
        }
        let rows = table[0] as usize;
        if 1 + rows * 6 > bytes as usize / 4 {
            return Err(failure("Termius 端口表不完整。"));
        }
        let result: Vec<u16> = table[1..1 + rows * 6]
            .chunks_exact(6)
            .filter(|row| {
                row[0] == 2 && row[1] == u32::from_ne_bytes([127, 0, 0, 1]) && row[5] == process
            })
            .map(|row| u16::from_be(row[2] as u16))
            .collect();
        if result.is_empty() || result.len() > 8 {
            return Err(failure(
                "Termius 的本机连接识别接口未启动，请以本机调试模式启动 Termius。",
            ));
        }
        Ok(result)
    }
    static VERIFIED: Mutex<Option<(u32, PathBuf, SystemTime, u64)>> = Mutex::new(None);
    fn verify_adapter(process: u32) -> AppResult<()> {
        let executable = crate::platform::process_executable(process)
            .ok_or_else(|| failure("无法核验 Termius 进程。"))?;
        if !executable
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Termius.exe"))
        {
            return Err(failure("连接接口进程不是 Termius。"));
        }
        let artifact = executable
            .parent()
            .ok_or_else(|| failure("Termius 安装路径无效。"))?
            .join("resources/app.asar");
        let metadata =
            std::fs::metadata(&artifact).map_err(|_| failure("无法核验 Termius 版本。"))?;
        let identity = (
            process,
            artifact.clone(),
            metadata
                .modified()
                .map_err(|_| failure("无法核验 Termius 版本时间。"))?,
            metadata.len(),
        );
        let mut verified = VERIFIED.lock().unwrap();
        if verified.as_ref() == Some(&identity) {
            return Ok(());
        }
        let mut file = File::open(&artifact).map_err(|_| failure("无法读取 Termius 版本标识。"))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|_| failure("无法读取 Termius 版本标识。"))?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        let hash = format!("{:x}", hasher.finalize());
        if hash != "dc2f4bf69399bf0093f50143702e9760073d1df53b5bf795a6d6849741d0f08c" {
            return Err(failure("当前 Termius 版本尚未验证，已停止自动识别。"));
        }
        *verified = Some(identity);
        Ok(())
    }
    pub fn capture(process: u32) -> AppResult<Connection> {
        verify_adapter(process)?;
        let mut focused = None;
        let mut last_error = None;
        for port in ports(process)? {
            let http = Http::open(port)?;
            let inventory = match http.json("/json/list") {
                Ok(value) => value,
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };
            let targets = inventory
                .as_array()
                .ok_or_else(|| failure("Termius 窗口列表无效。"))?;
            let mut count = 0;
            for target in targets {
                if target["type"] != "page" {
                    continue;
                }
                let Some(url) = target["url"].as_str() else {
                    continue;
                };
                let path = url.split(['?', '#']).next().unwrap_or("");
                if !path.starts_with("file:///")
                    || !path.ends_with("/ui-process/src/ui-process/main.html")
                {
                    continue;
                }
                count += 1;
                if count > 8 {
                    return Err(failure("Termius 窗口超过读取上限。"));
                }
                let id = target["id"]
                    .as_str()
                    .filter(|id| {
                        !id.is_empty()
                            && id.len() <= 128
                            && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                    })
                    .ok_or_else(|| failure("Termius 窗口标识无效。"))?;
                // Construct our own socket path; never follow a returned host or redirect.
                if let Some(connection) =
                    observation(http.probe(&format!("/devtools/page/{id}"))?, id)?
                {
                    if focused.replace(connection).is_some() {
                        return Err(failure("存在多个焦点终端，已停止自动上传。"));
                    }
                }
            }
        }
        focused.ok_or_else(|| {
            last_error.unwrap_or_else(|| failure("请将 Termius 服务器终端置于前台并点击输入区域。"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observation_rejects_unsafe_or_missing_connection_data() {
        let valid = serde_json::json!({"status":"connection-found","terminalId":"terminal","connectionId":"connection","protocol":"ssh","host":"192.0.2.1","port":22,"username":"user","broadcastChecked":true});
        assert!(observation(valid.clone(), "page").unwrap().is_some());
        for field in ["host", "username", "terminalId", "connectionId"] {
            let mut invalid = valid.clone();
            invalid[field] = "".into();
            assert!(observation(invalid, "page").is_err());
        }
        let mut unsafe_value = valid.clone();
        unsafe_value["broadcastChecked"] = false.into();
        assert!(observation(unsafe_value, "page").is_err());
        assert!(
            observation(serde_json::json!({"status":"not-focused"}), "page")
                .unwrap()
                .is_none()
        );
        assert!(observation(serde_json::json!({"status":"broadcast-active"}), "page").is_err());
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "显式选择 Termius 进程并保持终端焦点，仅验证原生只读连接读取，不上传或输入"]
    fn live_connection_readonly() {
        let process: u32 = std::env::var("TERMIUS_PROCESS_ID")
            .unwrap()
            .parse()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut last_error = None;
        while std::time::Instant::now() < deadline {
            match capture(process) {
                Ok(connection) => {
                    assert!(!connection.host.is_empty());
                    return;
                }
                Err(error) => {
                    last_error = Some(error);
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
        }
        panic!("{}", last_error.unwrap().message);
    }
}
