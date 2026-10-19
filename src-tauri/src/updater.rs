//! Portable Windows updates, staged beside the executable and applied after exit.
#![cfg_attr(not(windows), allow(dead_code))]
use crate::core::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "https://github.com/vipxlm/termius-image-paste";
const LATEST: &str = "https://api.github.com/repos/vipxlm/termius-image-paste/releases/latest";
const ASSET: &str = "TermiusImagePaste-windows-x86_64.exe";
const MAX_BINARY: u64 = 128 * 1024 * 1024;

fn failure(message: &str) -> AppError {
    AppError::new("update_failed", message)
}
fn io_error(_: std::io::Error) -> AppError {
    failure("无法读写更新文件，请检查程序目录权限和磁盘空间。")
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: String,
    pub current_version: String,
    pub available_version: Option<String>,
    pub message: String,
}
impl Status {
    pub fn new(state: &str, message: &str) -> Self {
        Self {
            state: state.into(),
            current_version: CURRENT.into(),
            available_version: None,
            message: message.into(),
        }
    }
}
pub fn supported() -> bool {
    cfg!(all(windows, target_arch = "x86_64", not(debug_assertions)))
}
pub fn initial() -> Status {
    if supported() {
        if let Ok(target) = std::env::current_exe() {
            if let Ok(Some(plan)) = read_plan(&target) {
                if let Some(message) = plan.failure {
                    return Status::new("error", &format!("上次更新未应用：{message}"));
                }
            }
        }
        Status::new("idle", "启动后后台检查 GitHub 正式版，下载完成后重启生效。")
    } else {
        Status::new(
            "unsupported",
            "在线更新适用于 Windows x64 便携正式版；当前版本请手动更新。",
        )
    }
}
fn version(value: &str) -> AppResult<[u64; 3]> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3 {
        return Err(failure("GitHub 版本号无效。"));
    }
    let mut result = [0; 3];
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty()
            || (part.len() > 1 && part.starts_with('0'))
            || !part.bytes().all(|c| c.is_ascii_digit())
        {
            return Err(failure("仅接受稳定版本号。"));
        }
        result[index] = part.parse().map_err(|_| failure("GitHub 版本号过大。"))?;
    }
    Ok(result)
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}
struct Candidate {
    version: String,
    binary: Asset,
    checksum: Asset,
    hash: String,
}
fn digest(value: &str) -> AppResult<String> {
    let hex = value.strip_prefix("sha256:").unwrap_or(value);
    if hex.len() != 64 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(failure("更新缺少有效的 SHA-256 校验值。"));
    }
    Ok(hex.to_ascii_lowercase())
}
fn select(bytes: &[u8], current: &str) -> AppResult<Option<Candidate>> {
    let release: Release =
        serde_json::from_slice(bytes).map_err(|_| failure("GitHub 版本响应无效。"))?;
    if release.draft || release.prerelease {
        return Err(failure("不安装草稿或预发布版本。"));
    }
    let latest = version(&release.tag_name)?;
    if latest <= version(current)? {
        return Ok(None);
    }
    if release.tag_name != format!("v{}.{}.{}", latest[0], latest[1], latest[2]) {
        return Err(failure("发布标签格式不受支持。"));
    }
    let asset = |name: &str| -> AppResult<Asset> {
        let matches: Vec<_> = release
            .assets
            .iter()
            .filter(|asset| asset.name == name)
            .collect();
        if matches.len() != 1 {
            return Err(failure("GitHub 发布资产不完整或存在重名。"));
        }
        let asset = matches[0];
        if asset.browser_download_url
            != format!("{REPO}/releases/download/{}/{name}", release.tag_name)
            || asset.size == 0
            || asset.size > MAX_BINARY
        {
            return Err(failure("GitHub 发布资产的地址或大小无效。"));
        }
        Ok(asset.clone())
    };
    let binary = asset(ASSET)?;
    let checksum = asset("SHA256SUMS.txt")?;
    let hash = digest(
        binary
            .digest
            .as_deref()
            .ok_or_else(|| failure("GitHub 发布资产缺少摘要，已停止更新。"))?,
    )?;
    Ok(Some(Candidate {
        version: format!("{}.{}.{}", latest[0], latest[1], latest[2]),
        binary,
        checksum,
        hash,
    }))
}
fn manifest_hash(bytes: &[u8]) -> AppResult<String> {
    let text = std::str::from_utf8(bytes).map_err(|_| failure("更新校验清单编码无效。"))?;
    let mut found = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        if name.trim_start_matches('*') == ASSET {
            if found.is_some() || parts.next().is_some() {
                return Err(failure("更新校验清单存在重复或无效记录。"));
            }
            found = Some(digest(hash)?);
        }
    }
    found.ok_or_else(|| failure("更新校验清单没有 Windows 程序。"))
}
pub(crate) fn hash_file(path: &Path) -> AppResult<String> {
    let mut file = File::open(path).map_err(io_error)?;
    if !file.metadata().map_err(io_error)?.is_file()
        || file.metadata().map_err(io_error)?.len() > MAX_BINARY
    {
        return Err(failure("更新程序类型或大小无效。"));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(io_error)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn verify_pe(path: &Path) -> AppResult<()> {
    let mut file = File::open(path).map_err(io_error)?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header)
        .map_err(|_| failure("更新文件不是有效的 Windows 程序。"))?;
    if &header[..2] != b"MZ" {
        return Err(failure("更新文件不是 Windows 程序。"));
    }
    let offset = u32::from_le_bytes(header[60..64].try_into().unwrap()) as u64;
    if !(64..=1024 * 1024).contains(&offset) {
        return Err(failure("更新程序头无效。"));
    }
    file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
    let mut pe = [0; 6];
    file.read_exact(&mut pe).map_err(io_error)?;
    if pe != [b'P', b'E', 0, 0, 0x64, 0x86] {
        return Err(failure("更新程序不是 Windows x64 版本。"));
    }
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
struct Plan {
    target: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
    helper: PathBuf,
    old_hash: String,
    new_hash: String,
    version: String,
    owner_pid: u32,
    phase: String,
    failure: Option<String>,
}
fn plan_path(target: &Path) -> PathBuf {
    target.with_extension("termius-update.json")
}
fn read_plan(target: &Path) -> AppResult<Option<Plan>> {
    let path = plan_path(target);
    if !path.exists() {
        return Ok(None);
    }
    let file = File::open(path).map_err(io_error)?;
    let plan: Plan = serde_json::from_reader(file.take(32769))
        .map_err(|_| failure("待更新记录无效，已保留现有程序。"))?;
    validate_plan(&plan, target)?;
    Ok(Some(plan))
}
fn validate_plan(plan: &Plan, target: &Path) -> AppResult<()> {
    if !target.is_absolute()
        || plan.target != target
        || plan.owner_pid == 0
        || version(&plan.version).is_err()
        || digest(&plan.old_hash).is_err()
        || digest(&plan.new_hash).is_err()
        || !["ready", "applying", "failed", "applied"].contains(&plan.phase.as_str())
    {
        return Err(failure("更新记录与当前程序不匹配。"));
    }
    for (path, prefix, suffix) in [
        (&plan.staged, ".termius-update-", ".bin"),
        (&plan.backup, ".termius-backup-", ".exe"),
        (&plan.helper, ".termius-helper-", ".exe"),
    ] {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let id = name
            .strip_prefix(prefix)
            .and_then(|s| s.strip_suffix(suffix))
            .unwrap_or("");
        if path.parent() != target.parent()
            || id.len() != 32
            || !id.bytes().all(|c| c.is_ascii_hexdigit())
            || path == target
        {
            return Err(failure(
                "更新文件必须位于程序目录，且具有有效的专用文件名。",
            ));
        }
    }
    if plan.backup == plan.helper {
        return Err(failure("更新备份路径无效。"));
    }
    Ok(())
}
fn save_plan(plan: &Plan) -> AppResult<()> {
    validate_plan(plan, &plan.target)?;
    let mut file =
        tempfile::NamedTempFile::new_in(plan.target.parent().unwrap()).map_err(io_error)?;
    serde_json::to_writer(file.as_file_mut(), plan).map_err(|_| failure("无法保存更新记录。"))?;
    file.as_file_mut().sync_all().map_err(io_error)?;
    file.persist(plan_path(&plan.target))
        .map_err(|e| io_error(e.error))?;
    Ok(())
}
fn clean_finished(target: &Path) -> AppResult<Option<Plan>> {
    let Some(plan) = read_plan(target)? else {
        return Ok(None);
    };
    if hash_file(target)? == plan.new_hash && plan.phase == "applied" {
        if let Err(error) = fs::remove_file(&plan.helper) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Ok(Some(plan));
            }
        }
        fs::remove_file(plan_path(target)).map_err(io_error)?;
        return Ok(None);
    }
    Ok(Some(plan))
}
#[cfg(windows)]
fn download(target: &Path, current: &str, mut progress: impl FnMut(Status)) -> AppResult<Status> {
    if let Some(plan) = clean_finished(target)? {
        if plan.phase == "applied" && hash_file(target)? == plan.new_hash {
            return Ok(Status::new(
                "current",
                "更新已生效，正在等待后台助手退出后清理临时文件。",
            ));
        }
        if plan.phase == "ready" {
            if hash_file(&plan.staged)? != plan.new_hash || hash_file(target)? != plan.old_hash {
                return Err(failure("待更新文件或当前程序已变化，已停止更新。"));
            }
            let mut status = Status::new("ready", "新版本已准备好，重启客户端即可生效。");
            status.available_version = Some(plan.version);
            return Ok(status);
        }
        if plan.phase == "applying" && process_running(plan.owner_pid) {
            return Err(failure("更新替换正在进行，请稍后重试。"));
        }
        if hash_file(target)? != plan.old_hash {
            return Err(failure("当前程序已变化，请重新下载正式版。"));
        }
        // Only remove this updater's validated staging files; old backups are retained.
        if plan.staged.exists() && hash_file(&plan.staged)? == plan.new_hash {
            fs::remove_file(&plan.staged).map_err(io_error)?;
        }
        if plan.helper.exists() && hash_file(&plan.helper)? == plan.old_hash {
            let _ = fs::remove_file(&plan.helper);
        }
        fs::remove_file(plan_path(target)).map_err(io_error)?;
    }
    let mut metadata = Vec::new();
    crate::update_http::fetch(LATEST, 1024 * 1024, &mut metadata)?;
    let Some(candidate) = select(&metadata, current)? else {
        return Ok(Status::new("current", "当前已是最新正式版。"));
    };
    let mut status = Status::new("downloading", "发现新版本，正在后台下载并校验…");
    status.available_version = Some(candidate.version.clone());
    progress(status);
    let parent = target.parent().ok_or_else(|| failure("程序路径无效。"))?;
    let mut binary = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    let count = crate::update_http::fetch(
        &candidate.binary.browser_download_url,
        candidate.binary.size,
        &mut binary,
    )?;
    binary.as_file_mut().sync_all().map_err(io_error)?;
    let mut checksums = Vec::new();
    crate::update_http::fetch(
        &candidate.checksum.browser_download_url,
        65536,
        &mut checksums,
    )?;
    if count != candidate.binary.size
        || hash_file(binary.path())? != candidate.hash
        || manifest_hash(&checksums)? != candidate.hash
    {
        return Err(failure("新版本大小或 SHA-256 校验失败，旧程序保持可用。"));
    }
    verify_pe(binary.path())?;
    let old_hash = hash_file(target)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut helper = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    std::io::copy(&mut File::open(target).map_err(io_error)?, &mut helper).map_err(io_error)?;
    helper.as_file_mut().sync_all().map_err(io_error)?;
    if hash_file(helper.path())? != old_hash {
        return Err(failure("当前程序发生变化，更新已停止。"));
    }
    let plan = Plan {
        target: target.into(),
        staged: parent.join(format!(".termius-update-{id}.bin")),
        backup: parent.join(format!(".termius-backup-{id}.exe")),
        helper: parent.join(format!(".termius-helper-{id}.exe")),
        old_hash,
        new_hash: candidate.hash,
        version: candidate.version.clone(),
        owner_pid: std::process::id(),
        phase: "ready".into(),
        failure: None,
    };
    binary
        .persist_noclobber(&plan.staged)
        .map_err(|e| io_error(e.error))?;
    if let Err(e) = helper.persist_noclobber(&plan.helper) {
        let _ = fs::remove_file(&plan.staged);
        return Err(io_error(e.error));
    }
    if let Err(e) = save_plan(&plan) {
        let _ = fs::remove_file(&plan.staged);
        let _ = fs::remove_file(&plan.helper);
        return Err(e);
    }
    let mut status = Status::new("ready", "新版本已下载并校验，重启客户端即可生效。");
    status.available_version = Some(candidate.version);
    Ok(status)
}
fn swap(plan: &Plan) -> AppResult<()> {
    if plan.backup.exists() {
        return Err(failure("更新备份已存在，停止以避免覆盖。"));
    }
    fs::rename(&plan.target, &plan.backup).map_err(io_error)?;
    if let Err(error) = fs::rename(&plan.staged, &plan.target) {
        fs::rename(&plan.backup, &plan.target)
            .map_err(|_| failure("更新替换失败，旧程序仍保存在备份文件中。"))?;
        return Err(io_error(error));
    }
    if hash_file(&plan.target).as_ref().ok() != Some(&plan.new_hash) {
        fs::rename(&plan.target, &plan.staged).map_err(io_error)?;
        fs::rename(&plan.backup, &plan.target).map_err(io_error)?;
        return Err(failure("替换后校验失败，已回退到旧版。"));
    }
    Ok(())
}
#[cfg(windows)]
fn wait_parent(pid: u32, timeout: u32) -> AppResult<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError},
        System::Threading::{OpenProcess, WaitForSingleObject},
    };
    unsafe {
        let process = OpenProcess(0x00100000, 0, pid);
        if process.is_null() {
            return if GetLastError() == 87 {
                Ok(())
            } else {
                Err(failure("无法确认旧进程是否退出。"))
            };
        }
        let result = WaitForSingleObject(process, timeout);
        CloseHandle(process);
        if result == 0 {
            Ok(())
        } else {
            Err(failure("旧进程尚未退出，本次未替换程序。"))
        }
    }
}
#[cfg(windows)]
fn process_running(pid: u32) -> bool {
    wait_parent(pid, 0).is_err()
}
#[cfg(windows)]
fn command(path: &Path) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(path);
    cmd.creation_flags(0x08000000);
    cmd
}
#[cfg(windows)]
fn launch(plan: &Plan, restart: bool) -> AppResult<()> {
    command(&plan.helper)
        .arg("--apply-update")
        .arg(plan_path(&plan.target))
        .arg(if restart { "restart" } else { "exit" })
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(io_error)?;
    Ok(())
}
static SCHEDULED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn schedule(restart: bool) -> AppResult<bool> {
    #[cfg(windows)]
    {
        use std::sync::atomic::Ordering;
        let target = std::env::current_exe().map_err(io_error)?;
        let Some(mut plan) = read_plan(&target)? else {
            return Ok(false);
        };
        if plan.phase != "ready" {
            return Ok(false);
        }
        if hash_file(&target)? != plan.old_hash
            || hash_file(&plan.staged)? != plan.new_hash
            || hash_file(&plan.helper)? != plan.old_hash
        {
            return Err(failure("更新文件已变化，未启动替换。"));
        }
        verify_pe(&plan.staged)?;
        if SCHEDULED.swap(true, Ordering::SeqCst) {
            return Ok(true);
        }
        plan.owner_pid = std::process::id();
        if let Err(error) = save_plan(&plan).and_then(|_| launch(&plan, restart)) {
            SCHEDULED.store(false, Ordering::SeqCst);
            return Err(error);
        }
        Ok(true)
    }
    #[cfg(not(windows))]
    {
        let _ = restart;
        Ok(false)
    }
}
/// Runs before Tauri and the single-instance plugin. The helper never opens a UI.
pub fn entry() -> Option<i32> {
    #[cfg(windows)]
    {
        let args: Vec<_> = std::env::args_os().collect();
        if args.get(1).is_some_and(|a| a == "--apply-update") {
            let run = || -> AppResult<()> {
                if args.len() != 4 || !["exit", "restart"].iter().any(|v| args[3] == *v) {
                    return Err(failure("更新助手参数无效。"));
                }
                let path = PathBuf::from(&args[2]);
                let file = File::open(&path).map_err(io_error)?;
                let mut plan: Plan = serde_json::from_reader(file.take(32769))
                    .map_err(|_| failure("更新记录无效。"))?;
                validate_plan(&plan, &plan.target)?;
                if path != plan_path(&plan.target)
                    || std::env::current_exe().map_err(io_error)? != plan.helper
                    || plan.phase != "ready"
                {
                    return Err(failure("更新助手与待更新记录不匹配。"));
                }
                let result = (|| {
                    wait_parent(plan.owner_pid, 60000)?;
                    if hash_file(&plan.target)? != plan.old_hash
                        || hash_file(&plan.staged)? != plan.new_hash
                    {
                        return Err(failure("更新前文件已变化，保留当前程序。"));
                    }
                    verify_pe(&plan.staged)?;
                    plan.phase = "applying".into();
                    save_plan(&plan)?;
                    // Windows can briefly retain executable image mappings after process exit.
                    let mut last = None;
                    for _ in 0..30 {
                        match swap(&plan) {
                            Ok(()) => {
                                last = None;
                                break;
                            }
                            Err(e) => last = Some(e),
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    if let Some(error) = last {
                        return Err(error);
                    }
                    plan.phase = "applied".into();
                    save_plan(&plan)?;
                    Ok(())
                })();
                if let Err(error) = &result {
                    plan.phase = "failed".into();
                    plan.failure = Some(error.message.clone());
                    let _ = save_plan(&plan);
                }
                if args[3] == "restart" && plan.target.is_file() {
                    command(&plan.target).spawn().map_err(io_error)?;
                }
                result
            };
            return Some(if run().is_ok() { 0 } else { 1 });
        }
        if !cfg!(debug_assertions) {
            if let Ok(target) = std::env::current_exe() {
                if let Ok(Some(plan)) = clean_finished(&target) {
                    if plan.phase == "ready"
                        && !process_running(plan.owner_pid)
                        && schedule(true).unwrap_or(false)
                    {
                        return Some(0);
                    }
                }
            }
        }
    }
    None
}
pub fn check(app: tauri::AppHandle) -> AppResult<()> {
    let state = crate::state(&app);
    if !supported() {
        return Err(failure("当前仅 Windows x64 便携正式版支持在线更新。"));
    }
    if state
        .update_checking
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return Ok(());
    }
    {
        let mut status = state.update.lock().unwrap();
        let available_version = status.available_version.clone();
        *status = Status::new("checking", "正在后台检查 GitHub 最新正式版…");
        status.available_version = available_version;
    }
    crate::publish(&app);
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(windows)]
        let result = std::env::current_exe()
            .map_err(io_error)
            .and_then(|target| {
                download(&target, CURRENT, |status| {
                    *state.update.lock().unwrap() = status;
                    crate::publish(&app);
                })
            });
        #[cfg(not(windows))]
        let result: AppResult<Status> = Err(failure("当前平台不支持静默更新。"));
        {
            let mut status = state.update.lock().unwrap();
            *status = result.unwrap_or_else(|error| {
                let mut failed = Status::new("error", &error.message);
                failed.available_version = status.available_version.clone();
                failed
            });
        }
        state
            .update_checking
            .store(false, std::sync::atomic::Ordering::SeqCst);
        crate::publish(&app);
    });
    Ok(())
}
pub fn start(app: tauri::AppHandle) {
    if !supported() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        loop {
            let _ = check(app.clone());
            tokio::time::sleep(std::time::Duration::from_secs(6 * 60 * 60)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata(tag: &str) -> serde_json::Value {
        serde_json::json!({"tag_name":tag,"draft":false,"prerelease":false,"assets":[
            {"name":ASSET,"browser_download_url":format!("{REPO}/releases/download/{tag}/{ASSET}"),"size":12345,"digest":format!("sha256:{}","a".repeat(64))},
            {"name":"SHA256SUMS.txt","browser_download_url":format!("{REPO}/releases/download/{tag}/SHA256SUMS.txt"),"size":700}
        ]})
    }
    fn selection(value: &serde_json::Value, current: &str) -> AppResult<Option<Candidate>> {
        select(&serde_json::to_vec(value).unwrap(), current)
    }
    #[test]
    fn versions_are_numeric_and_never_downgrade() {
        assert!(selection(&metadata("v2.10.0"), "2.9.9").unwrap().is_some());
        assert!(selection(&metadata("v2.2.0"), "2.3.0").unwrap().is_none());
        assert!(selection(&metadata("v2.3.0"), "2.3.0").unwrap().is_none());
        for value in [
            "2.3",
            "2.3.0-beta.1",
            "2.03.0",
            "2.3.-1",
            "v2.3.0/escape",
            "18446744073709551616.0.0",
        ] {
            assert!(version(value).is_err(), "{value}");
        }
    }
    #[test]
    fn candidate_requires_stable_unique_trusted_assets_and_digest() {
        for field in ["draft", "prerelease"] {
            let mut m = metadata("v3.0.0");
            m[field] = true.into();
            assert!(selection(&m, CURRENT).is_err());
        }
        let mut m = metadata("v3.0.0");
        m["assets"][0]["browser_download_url"] = "https://example.com/program.exe".into();
        assert!(selection(&m, CURRENT).is_err());
        let mut m = metadata("v3.0.0");
        m["assets"][0]["digest"] = serde_json::Value::Null;
        assert!(selection(&m, CURRENT).is_err());
        let mut m = metadata("v3.0.0");
        m["assets"][0]["size"] = (MAX_BINARY + 1).into();
        assert!(selection(&m, CURRENT).is_err());
        let mut m = metadata("v3.0.0");
        let duplicate = m["assets"][0].clone();
        m["assets"].as_array_mut().unwrap().push(duplicate);
        assert!(selection(&m, CURRENT).is_err());
        assert!(select(b"not json", CURRENT).is_err());
    }
    #[test]
    fn manifest_requires_one_valid_binary_hash() {
        let hash = "a".repeat(64);
        assert_eq!(
            manifest_hash(format!("{hash}  {ASSET}\n").as_bytes()).unwrap(),
            hash
        );
        assert!(manifest_hash(format!("{hash} {ASSET}\n{hash} {ASSET}").as_bytes()).is_err());
        assert!(manifest_hash(b"bad data").is_err());
        assert!(manifest_hash(format!("INVALID {ASSET}").as_bytes()).is_err());
    }
    fn plan(root: &Path) -> Plan {
        let id = "a".repeat(32);
        let target = root.join("client.exe");
        let staged = root.join(format!(".termius-update-{id}.bin"));
        fs::write(&target, b"old application").unwrap();
        fs::write(&staged, b"new application").unwrap();
        Plan {
            target: target.clone(),
            staged: staged.clone(),
            backup: root.join(format!(".termius-backup-{id}.exe")),
            helper: root.join(format!(".termius-helper-{id}.exe")),
            old_hash: hash_file(&target).unwrap(),
            new_hash: hash_file(&staged).unwrap(),
            version: "3.0.0".into(),
            owner_pid: 12345,
            phase: "ready".into(),
            failure: None,
        }
    }
    #[test]
    fn swap_keeps_backup_and_rolls_back_on_failure() {
        let root = tempfile::tempdir().unwrap();
        let p = plan(root.path());
        swap(&p).unwrap();
        assert_eq!(fs::read(&p.target).unwrap(), b"new application");
        assert_eq!(fs::read(&p.backup).unwrap(), b"old application");
        let root = tempfile::tempdir().unwrap();
        let p = plan(root.path());
        fs::remove_file(&p.staged).unwrap();
        assert!(swap(&p).is_err());
        assert_eq!(fs::read(&p.target).unwrap(), b"old application");
        assert!(!p.backup.exists());
        let root = tempfile::tempdir().unwrap();
        let mut p = plan(root.path());
        p.new_hash = "b".repeat(64);
        assert!(swap(&p).is_err());
        assert_eq!(fs::read(&p.target).unwrap(), b"old application");
    }
    #[test]
    fn plans_reject_foreign_paths_and_backup_is_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let mut p = plan(root.path());
        validate_plan(&p, &p.target).unwrap();
        fs::write(&p.backup, b"existing user file").unwrap();
        assert!(swap(&p).is_err());
        assert_eq!(fs::read(&p.backup).unwrap(), b"existing user file");
        assert_eq!(fs::read(&p.target).unwrap(), b"old application");
        p.staged = root.path().join("user-document");
        assert!(validate_plan(&p, &p.target).is_err());
        assert!(verify_pe(&p.target).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn redirects_require_exact_github_https_hosts() {
        use crate::update_http::endpoint;
        for url in [
            "http://github.com/file",
            "https://github.com.evil.invalid/file",
            "https://user@github.com/file",
            "https://github.com:443/file",
            "https://github.com/a#b",
            "https://github.com/a\r\nHeader:evil",
        ] {
            assert!(endpoint(url).is_err(), "{url}");
        }
        assert!(
            endpoint("https://release-assets.githubusercontent.com/asset?token=redacted").is_ok()
        );
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "显式启用：从 GitHub 下载正式资产到临时目录，校验并暂存；不替换或启动任何客户端"]
    fn live_github_download_to_temporary_directory() {
        let root = tempfile::tempdir().unwrap();
        let p = plan(root.path());
        let original = fs::read(&p.target).unwrap();
        let status = download(&p.target, "0.0.0", |_| {}).unwrap();
        assert_eq!(status.state, "ready");
        let plan = read_plan(&p.target).unwrap().unwrap();
        assert_eq!(hash_file(&plan.staged).unwrap(), plan.new_hash);
        verify_pe(&plan.staged).unwrap();
        assert_eq!(fs::read(&p.target).unwrap(), original);
        assert!(!plan.backup.exists());
    }
}
