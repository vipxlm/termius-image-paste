//! Windows HTTPS transport. Only GitHub and its release CDN are allowed.
use crate::core::{AppError, AppResult};
use std::{
    ffi::c_void,
    io::Write,
    ptr,
    time::{Duration, Instant},
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
    fn WinHttpSetOption(handle: Handle, option: u32, buffer: *const c_void, length: u32) -> i32;
    fn WinHttpSendRequest(
        handle: Handle,
        headers: *const u16,
        length: u32,
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
    fn WinHttpReadData(handle: Handle, buffer: *mut c_void, length: u32, read: *mut u32) -> i32;
    fn WinHttpCloseHandle(handle: Handle) -> i32;
}
struct Internet(Handle);
impl Drop for Internet {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}
fn fail(message: &str) -> AppError {
    AppError::new("update_network", message)
}
fn check(ok: i32) -> AppResult<()> {
    if ok == 0 {
        Err(fail("无法连接 GitHub 或请求超时，请稍后重试。"))
    } else {
        Ok(())
    }
}
fn own(handle: Handle) -> AppResult<Internet> {
    if handle.is_null() {
        Err(fail("无法建立更新下载连接。"))
    } else {
        Ok(Internet(handle))
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub(crate) fn endpoint(url: &str) -> AppResult<(&str, String)> {
    let (host, path) = url
        .strip_prefix("https://")
        .and_then(|u| u.split_once('/'))
        .ok_or_else(|| fail("更新下载地址必须使用 HTTPS。"))?;
    if ![
        "api.github.com",
        "github.com",
        "release-assets.githubusercontent.com",
        "objects.githubusercontent.com",
    ]
    .contains(&host)
        || url.chars().any(char::is_control)
        || path.contains(['\\', '#'])
    {
        return Err(fail("更新下载地址不属于允许的 GitHub 域名。"));
    }
    Ok((host, format!("/{path}")))
}
pub fn fetch(url: &str, limit: u64, output: &mut impl Write) -> AppResult<u64> {
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut url = url.to_string();
    unsafe {
        let session = own(WinHttpOpen(
            wide("TermiusImagePaste-Updater/1").as_ptr(),
            4,
            ptr::null(),
            ptr::null(),
            0,
        ))?;
        check(WinHttpSetTimeouts(session.0, 10000, 15000, 20000, 20000))?;
        for _ in 0..5 {
            let (host, path) = endpoint(&url)?;
            let connection = own(WinHttpConnect(session.0, wide(host).as_ptr(), 443, 0))?;
            let request = own(WinHttpOpenRequest(
                connection.0,
                wide("GET").as_ptr(),
                wide(&path).as_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                0x00800000,
            ))?;
            let never: u32 = 0;
            check(WinHttpSetOption(request.0, 88, &never as *const _ as _, 4))?;
            let headers = wide("Accept: application/vnd.github+json\r\n");
            check(WinHttpSendRequest(
                request.0,
                headers.as_ptr(),
                u32::MAX,
                ptr::null(),
                0,
                0,
                0,
            ))?;
            check(WinHttpReceiveResponse(request.0, ptr::null_mut()))?;
            let mut status = 0u32;
            let mut size = 4;
            check(WinHttpQueryHeaders(
                request.0,
                19 | 0x20000000,
                ptr::null(),
                &mut status as *mut _ as _,
                &mut size,
                ptr::null_mut(),
            ))?;
            if [301, 302, 303, 307, 308].contains(&status) {
                let mut buffer = vec![0u16; 8192];
                let mut size = (buffer.len() * 2) as u32;
                check(WinHttpQueryHeaders(
                    request.0,
                    33,
                    ptr::null(),
                    buffer.as_mut_ptr() as _,
                    &mut size,
                    ptr::null_mut(),
                ))?;
                let end = buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len());
                url = String::from_utf16(&buffer[..end])
                    .map_err(|_| fail("GitHub 下载跳转无效。"))?;
                continue;
            }
            if status != 200 {
                return Err(fail(&format!("GitHub 返回 HTTP {status}，请稍后重试。")));
            }
            let mut total = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                if Instant::now() > deadline {
                    return Err(fail("更新下载超时，旧版程序保持可用。"));
                }
                let mut count = 0;
                check(WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr() as _,
                    buffer.len() as u32,
                    &mut count,
                ))?;
                if count == 0 {
                    return Ok(total);
                }
                total += count as u64;
                if total > limit {
                    return Err(fail("更新响应超过大小上限。"));
                }
                output
                    .write_all(&buffer[..count as usize])
                    .map_err(|_| fail("无法保存更新，请检查程序目录写入权限和磁盘空间。"))?;
            }
        }
    }
    Err(fail("GitHub 下载跳转次数过多。"))
}
