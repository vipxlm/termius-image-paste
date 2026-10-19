mod core;
mod platform;
mod termius;
#[cfg(windows)]
mod update_http;
mod updater;
pub use updater::entry as update_entry;

use core::{AppError, AppResult, ServerProfile, Settings, SettingsStore, SshClient};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Operation {
    status: String,
    stage: String,
    message: String,
    target: Option<String>,
    cancellable: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LastResult {
    status: String,
    stage: String,
    message: String,
    path: Option<String>,
    target: Option<String>,
    at: String,
    details: Option<String>,
}
#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DetectedTarget {
    session: String,
    server_id: Option<String>,
    message: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeState {
    shortcut_status: String,
    operation: Option<Operation>,
    last_result: Option<LastResult>,
    detected_target: Option<DetectedTarget>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    settings: Settings,
    capabilities: platform::Capabilities,
    runtime: RuntimeState,
    settings_path: String,
    update: updater::Status,
}
#[derive(Clone)]
struct Ticket {
    id: u64,
    token: CancellationToken,
}
#[derive(Default)]
struct Hotkeys {
    requested: Vec<Shortcut>,
    registered: Vec<Shortcut>,
    status: String,
    dirty: bool,
}
struct AppState {
    settings: Mutex<Settings>,
    mutation: Mutex<()>,
    store: SettingsStore,
    ssh: SshClient,
    runtime: Mutex<RuntimeState>,
    busy: Mutex<Option<Ticket>>,
    bindings: Mutex<Hotkeys>,
    generation: AtomicU64,
    operation_id: AtomicU64,
    closing: AtomicBool,
    tray_available: AtomicBool,
    startup_error: Option<AppError>,
    update: Mutex<updater::Status>,
    update_checking: AtomicBool,
}
fn error(code: &str, message: &str, detail: impl ToString) -> AppError {
    AppError {
        code: code.into(),
        message: message.into(),
        detail: Some(detail.to_string()),
    }
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn target_name(p: &ServerProfile) -> String {
    format!("{} · {}@{}:{}", p.name, p.username, p.host, p.port)
}
fn state(app: &AppHandle) -> Arc<AppState> {
    app.state::<Arc<AppState>>().inner().clone()
}
fn snapshot(app: &AppHandle) -> Snapshot {
    let s = state(app);
    let mut caps = platform::capabilities();
    caps.tray_available = s.tray_available.load(Ordering::SeqCst);
    let settings = s.settings.lock().unwrap().clone();
    let runtime = s.runtime.lock().unwrap().clone();
    let update = s.update.lock().unwrap().clone();
    Snapshot {
        settings,
        capabilities: caps,
        runtime,
        settings_path: s.store.settings_path().to_string_lossy().into(),
        update,
    }
}
fn publish(app: &AppHandle) {
    let _ = app.emit("app-state", snapshot(app));
}
fn ensure_writable(s: &AppState) -> AppResult<()> {
    if let Some(e) = &s.startup_error {
        return Err(e.clone());
    }
    Ok(())
}
fn persist(app: &AppHandle, settings: Settings) -> AppResult<Settings> {
    let s = state(app);
    ensure_writable(&s)?;
    let saved = s.store.save(settings)?;
    *s.settings.lock().unwrap() = saved.clone();
    s.generation.fetch_add(1, Ordering::SeqCst);
    if let Some(job) = s.busy.lock().unwrap().as_ref() {
        job.token.cancel();
    }
    publish(app);
    Ok(saved)
}
fn begin(app: &AppHandle, stage: &str, message: &str, target: Option<String>) -> AppResult<Ticket> {
    let s = state(app);
    ensure_writable(&s)?;
    let mut busy = s.busy.lock().unwrap();
    if s.closing.load(Ordering::SeqCst) {
        return Err(AppError::new("closing", "客户端正在退出，请重启后操作。"));
    }
    if busy.is_some() {
        return Err(AppError::new(
            "operation_busy",
            "已有操作正在进行，请等待完成或先取消。",
        ));
    }
    let ticket = Ticket {
        id: s.operation_id.fetch_add(1, Ordering::SeqCst),
        token: CancellationToken::new(),
    };
    *busy = Some(ticket.clone());
    s.runtime.lock().unwrap().operation = Some(Operation {
        status: "running".into(),
        stage: stage.into(),
        message: message.into(),
        target,
        cancellable: true,
    });
    drop(busy);
    publish(app);
    Ok(ticket)
}
fn stage(app: &AppHandle, ticket: &Ticket, message: &str, name: &str) {
    let s = state(app);
    let busy = s.busy.lock().unwrap();
    if busy.as_ref().is_some_and(|b| b.id == ticket.id) {
        if let Some(op) = s.runtime.lock().unwrap().operation.as_mut() {
            op.message = message.into();
            op.stage = name.into();
        }
    }
    drop(busy);
    publish(app);
}
fn finish(app: &AppHandle, ticket: &Ticket, result: Option<LastResult>) {
    let s = state(app);
    let mut busy = s.busy.lock().unwrap();
    if busy.as_ref().is_some_and(|b| b.id == ticket.id) {
        *busy = None;
        let mut runtime = s.runtime.lock().unwrap();
        runtime.operation = None;
        if let Some(result) = result {
            runtime.last_result = Some(result);
        }
        drop(runtime);
        drop(busy);
        publish(app);
    }
}
#[tauri::command]
fn get_state(app: AppHandle) -> Snapshot {
    snapshot(&app)
}
#[tauri::command]
async fn save_server(app: AppHandle, profile: ServerProfile) -> AppResult<Settings> {
    tauri::async_runtime::spawn_blocking(move || {
        profile.validate(false)?;
        let s = state(&app);
        let _gate = s.mutation.lock().unwrap();
        let mut settings = s.settings.lock().unwrap().clone();
        if let Some(existing) = settings.servers.iter_mut().find(|p| p.id == profile.id) {
            *existing = profile;
        } else {
            settings.servers.push(profile);
        }
        persist(&app, settings)
    })
    .await
    .map_err(|e| error("internal", "保存服务器失败。", e))?
}
#[tauri::command]
async fn remove_server(app: AppHandle, id: String) -> AppResult<Settings> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = state(&app);
        let _gate = s.mutation.lock().unwrap();
        let mut settings = s.settings.lock().unwrap().clone();
        settings.servers.retain(|p| p.id != id);
        if settings.active_server_id.as_ref() == Some(&id) {
            settings.active_server_id = None;
        }
        persist(&app, settings)
    })
    .await
    .map_err(|e| error("internal", "删除服务器配置失败。", e))?
}
#[tauri::command]
async fn set_active_server(app: AppHandle, id: String) -> AppResult<Settings> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = state(&app);
        let _gate = s.mutation.lock().unwrap();
        let mut settings = s.settings.lock().unwrap().clone();
        let p = settings
            .servers
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| AppError::new("unknown_server", "服务器不存在。"))?;
        p.validate(true)?;
        settings.active_server_id = Some(id);
        s.runtime.lock().unwrap().detected_target = None;
        persist(&app, settings)
    })
    .await
    .map_err(|e| error("internal", "切换服务器失败。", e))?
}
#[tauri::command]
async fn set_enabled(app: AppHandle, enabled: bool) -> AppResult<Settings> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = state(&app);
        let _gate = s.mutation.lock().unwrap();
        let mut settings = s.settings.lock().unwrap().clone();
        settings.enabled = enabled;
        persist(&app, settings)
    })
    .await
    .map_err(|e| error("internal", "切换贴图状态失败。", e))?
}

fn parse_shortcut(value: &str) -> AppResult<Shortcut> {
    core::validate_shortcut(value)?;
    value
        .parse()
        .map_err(|e| error("invalid_shortcut", "快捷键格式无效。", e))
}
#[tauri::command]
async fn save_preferences(
    app: AppHandle,
    shortcut: String,
    legacy_shortcut_enabled: bool,
    close_to_tray: bool,
    notifications: bool,
) -> AppResult<Settings> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = state(&app);
        let _mutation = s.mutation.lock().unwrap();
        ensure_writable(&s)?;
        let primary = parse_shortcut(&shortcut)?;
        if platform::capabilities().hotkey_available {
            let mut gate = s.bindings.lock().unwrap();
            let old = gate.registered.clone();
            gate.dirty = true;
            app.global_shortcut()
                .unregister_all()
                .map_err(|e| error("hotkey_release", "无法释放原快捷键，设置未改变。", e))?;
            let checked = app.global_shortcut().register(primary);
            if checked.is_ok() {
                let _ = app.global_shortcut().unregister(primary);
            }
            for key in old {
                let _ = app.global_shortcut().register(key);
            }
            checked.map_err(|e| {
                error(
                    "hotkey_conflict",
                    "此快捷键不可用，请选择其他组合；原设置已保留。",
                    e,
                )
            })?;
        }
        let mut settings = s.settings.lock().unwrap().clone();
        settings.shortcut = shortcut;
        settings.legacy_shortcut_enabled = legacy_shortcut_enabled;
        settings.close_to_tray = close_to_tray;
        settings.notifications = notifications;
        persist(&app, settings)
    })
    .await
    .map_err(|e| error("internal", "保存快捷键失败。", e))?
}

#[derive(Serialize)]
struct ScannedKey {
    key: String,
    fingerprint: String,
}
#[tauri::command]
async fn scan_host_key(app: AppHandle, profile: ServerProfile) -> AppResult<ScannedKey> {
    let job = begin(
        &app,
        "fingerprint",
        "正在读取服务器主机指纹…",
        Some(target_name(&profile)),
    )?;
    let result = async {
        let key = state(&app)
            .ssh
            .scan_host_key(&profile, job.token.clone())
            .await?;
        let fingerprint = core::fingerprint(&key)?;
        Ok(ScannedKey { key, fingerprint })
    }
    .await;
    finish(&app, &job, None);
    result
}
#[tauri::command]
async fn login_server(
    app: AppHandle,
    mut profile: ServerProfile,
    password: Option<String>,
    use_default_directory: bool,
) -> AppResult<ServerProfile> {
    let job = begin(
        &app,
        "connecting",
        "正在登录并准备上传目录…",
        Some(target_name(&profile)),
    )?;
    let result = async {
        let s = state(&app);
        if let Some(password) = password {
            if profile.key_file.is_empty() {
                profile.key_file = s.ssh.generate_key(&profile, job.token.clone()).await?;
            }
            profile.validate(true)?;
            let exe = std::env::current_exe()
                .map_err(|e| error("askpass", "无法定位密码辅助程序。", e))?;
            s.ssh
                .authorize(&profile, password, &exe, job.token.clone())
                .await?;
        }
        let paths = s
            .ssh
            .remote_directory(
                &profile,
                if use_default_directory {
                    None
                } else {
                    Some(&profile.remote_directory)
                },
                false,
                job.token.clone(),
            )
            .await?;
        profile.remote_directory = paths[0].clone();
        s.ssh.test(&profile, job.token.clone()).await?;
        profile.verified_at = Some(now());
        Ok(profile)
    }
    .await;
    finish(&app, &job, None);
    result
}
#[tauri::command]
async fn browse_remote_directory(
    app: AppHandle,
    profile: ServerProfile,
    directory: Option<String>,
) -> AppResult<Vec<String>> {
    let job = begin(
        &app,
        "connecting",
        "正在读取远端文件夹…",
        Some(target_name(&profile)),
    )?;
    let result = state(&app)
        .ssh
        .remote_directory(&profile, directory.as_deref(), true, job.token.clone())
        .await;
    finish(&app, &job, None);
    result
}
#[tauri::command]
fn cancel_operation(app: AppHandle) {
    let s = state(&app);
    let busy = s.busy.lock().unwrap();
    if let Some(job) = busy.as_ref() {
        job.token.cancel();
        if let Some(op) = s.runtime.lock().unwrap().operation.as_mut() {
            op.message = "正在取消操作…".into();
            op.stage = "cancelling".into();
        }
    }
    drop(busy);
    publish(&app);
}

#[tauri::command]
async fn choose_key_file(app: AppHandle) -> AppResult<Option<String>> {
    choose_file(app).await
}
#[tauri::command]
async fn choose_upload_file(app: AppHandle) -> AppResult<Option<String>> {
    choose_file(app).await
}
async fn choose_file(app: AppHandle) -> AppResult<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .blocking_pick_file()
            .map(|p| {
                p.into_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .map_err(|e| error("file_path", "请选择本机文件。", e))
            })
            .transpose()
    })
    .await
    .map_err(|e| error("file_dialog", "无法打开文件选择窗口。", e))?
}
#[tauri::command]
fn copy_path(app: AppHandle, path: String) -> AppResult<()> {
    if !path.starts_with('/') || path.chars().any(char::is_control) || path.len() > 8192 {
        return Err(AppError::new("invalid_path", "文件路径无效。"));
    }
    app.clipboard().write_text(path).map_err(|e| {
        error(
            "clipboard_unavailable",
            "无法复制路径，可直接选中路径手动复制。",
            e,
        )
    })
}

enum UploadSource {
    Clipboard,
    File(PathBuf),
    Bytes(Vec<u8>),
    FileData { name: String, bytes: Vec<u8> },
}
#[tauri::command]
fn upload_clipboard(app: AppHandle) -> AppResult<()> {
    start_upload(app, UploadSource::Clipboard, None)
}
#[tauri::command]
fn upload_file(app: AppHandle, path: String) -> AppResult<()> {
    start_upload(app, UploadSource::File(PathBuf::from(path)), None)
}
#[tauri::command]
fn upload_image_data(app: AppHandle, data_url: String) -> AppResult<()> {
    use base64::Engine;
    if data_url.len() > 48 * 1024 * 1024 {
        return Err(AppError::new(
            "image_too_large",
            "图片过大，请使用较小截图（最多 32 MiB）。",
        ));
    }
    let (header, data) = data_url
        .split_once(',')
        .ok_or_else(|| AppError::new("invalid_image", "剪贴板图片格式无效。"))?;
    if ![
        "data:image/png;base64",
        "data:image/jpeg;base64",
        "data:image/webp;base64",
    ]
    .contains(&header)
    {
        return Err(AppError::new(
            "invalid_image",
            "仅支持 PNG、JPEG 和 WebP 图片。",
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| error("invalid_image", "图片编码无效。", e))?;
    start_upload(app, UploadSource::Bytes(bytes), None)
}
#[tauri::command]
fn upload_file_data(app: AppHandle, name: String, data_url: String) -> AppResult<()> {
    use base64::Engine;
    if data_url.len() > 48 * 1024 * 1024 {
        return Err(AppError::new(
            "file_too_large",
            "粘贴文件不能超过 32 MiB，请改用选择文件或拖入。",
        ));
    }
    let (_, data) = data_url
        .split_once(',')
        .filter(|(header, _)| header.starts_with("data:") && header.ends_with(";base64"))
        .ok_or_else(|| AppError::new("invalid_file", "粘贴文件编码无效。"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| error("invalid_file", "粘贴文件编码无效。", e))?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(AppError::new(
            "file_too_large",
            "粘贴文件不能超过 32 MiB，请改用选择文件或拖入。",
        ));
    }
    core::upload_filename(&name)?;
    start_upload(app, UploadSource::FileData { name, bytes }, None)
}
fn prompt_path(path: &str) -> String {
    if path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    {
        format!("{path} ")
    } else {
        format!("{} ", core::quote_posix(path))
    }
}
fn start_upload(
    app: AppHandle,
    source: UploadSource,
    target: Option<platform::Target>,
) -> AppResult<()> {
    let target = match target {
        Some(mut target) if cfg!(windows) => {
            target.terminal = Some(termius::capture(target.process)?);
            Some(target)
        }
        other => other,
    };
    let s = state(&app);
    let (profile, generation) = {
        let settings = s.settings.lock().unwrap();
        if target.is_some() && !settings.enabled {
            return Err(AppError::new("paused", "贴图快捷键已暂停。"));
        }
        let p = if platform::capabilities().automatic_target_selection {
            if let Some(target) = &target {
                core::server_for_connection(
                    &settings.servers,
                    target.terminal.as_ref().ok_or_else(|| {
                        AppError::new("connection_unavailable", "无法读取当前 Termius 真实连接。")
                    })?,
                )?
            } else if let Some(detected) = &s.runtime.lock().unwrap().detected_target {
                settings
                    .servers
                    .iter()
                    .find(|p| Some(&p.id) == detected.server_id.as_ref())
                    .ok_or_else(|| AppError::new("connection_unmatched", &detected.message))?
            } else {
                settings.active_server().ok_or_else(|| {
                    AppError::new("no_target", "请先打开已配置服务器的 Termius 会话。")
                })?
            }
        } else {
            settings
                .active_server()
                .ok_or_else(|| AppError::new("no_target", "请先选择当前上传服务器。"))?
        }
        .clone();
        (p, s.generation.load(Ordering::SeqCst))
    };
    profile.validate(true)?;
    let job = begin(
        &app,
        "clipboard",
        "正在读取图片或文件…",
        Some(target_name(&profile)),
    )?;
    tauri::async_runtime::spawn(async move {
        let result: AppResult<LastResult> = async {
            let handle = app.clone();
            let prepared =
                tauri::async_runtime::spawn_blocking(move || prepare_upload(&handle, source))
                    .await
                    .map_err(|e| error("image_prepare", "读取图片或文件失败。", e))??;
            if job.token.is_cancelled() {
                return Err(AppError::new("cancelled", "操作已取消。"));
            }
            if s.generation.load(Ordering::SeqCst) != generation
                || target
                    .as_ref()
                    .is_some_and(|target| !platform::target_unchanged(target))
            {
                return Err(AppError::new(
                    "target_changed",
                    "当前连接或设置已变化，已停止上传，请重新触发贴图。",
                ));
            }
            stage(&app, &job, "正在上传文件…", "upload");
            let remote = s
                .ssh
                .upload(&profile, &prepared.path, &prepared.name, job.token.clone())
                .await?;
            let mut status = "success";
            let mut message = "文件已上传。可选中或复制下方路径，在目标会话使用。".to_string();
            let mut result_stage = "uploaded";
            if let Some(target) = target {
                stage(&app, &job, "文件已上传，准备填入路径…", "input");
                for _ in 0..50 {
                    if !platform::modifiers_down() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                if job.token.is_cancelled()
                    || s.generation.load(Ordering::SeqCst) != generation
                    || !platform::target_unchanged(&target)
                {
                    status = "warning";
                    message = "文件已上传，但窗口或设置发生变化，未自动填入路径。".into();
                } else {
                    match platform::type_path(&target, &prompt_path(&remote)) {
                        Ok(()) => {
                            message = "文件已上传，路径已填入 Termius；由你发送消息。".into();
                            result_stage = "inserted"
                        }
                        Err(e) => {
                            status = "warning";
                            message = e;
                        }
                    }
                }
            }
            Ok(LastResult {
                status: status.into(),
                stage: result_stage.into(),
                message,
                path: Some(remote),
                target: Some(target_name(&profile)),
                at: now(),
                details: None,
            })
        }
        .await;
        let result = result.unwrap_or_else(|e| LastResult {
            status: if e.code == "cancelled" {
                "warning"
            } else {
                "error"
            }
            .into(),
            stage: "failed".into(),
            message: e.message,
            path: None,
            target: Some(target_name(&profile)),
            at: now(),
            details: e.detail,
        });
        let notifications = s.settings.lock().unwrap().notifications;
        let notification_message = result.message.clone();
        finish(&app, &job, Some(result));
        if notifications {
            tauri::async_runtime::spawn_blocking(move || {
                let _ = app
                    .notification()
                    .builder()
                    .title("Termius 贴图助手")
                    .body(notification_message)
                    .show();
            });
        }
    });
    Ok(())
}
struct PreparedUpload {
    path: PathBuf,
    name: String,
    _temporary: Option<tempfile::NamedTempFile>,
}
fn prepare_upload(app: &AppHandle, source: UploadSource) -> AppResult<PreparedUpload> {
    let image = match source {
        UploadSource::Clipboard => {
            if let Some(path) = platform::clipboard_file()? {
                return prepare_upload(app, UploadSource::File(path));
            }
            let clipboard = app.clipboard().read_image().map_err(|e| {
                error(
                    "clipboard_unavailable",
                    "剪贴板中没有可读取的图片或文件，请截图、复制单个文件或选择文件。",
                    e,
                )
            })?;
            if clipboard.width() > 16384
                || clipboard.height() > 16384
                || clipboard.rgba().len() > 128 * 1024 * 1024
            {
                return Err(AppError::new(
                    "image_too_large",
                    "截图尺寸过大，请缩小截图范围。",
                ));
            }
            image::RgbaImage::from_raw(
                clipboard.width(),
                clipboard.height(),
                clipboard.rgba().to_vec(),
            )
            .map(image::DynamicImage::ImageRgba8)
            .ok_or_else(|| AppError::new("invalid_image", "剪贴板图片数据不完整。"))?
        }
        UploadSource::File(path) => {
            if !path.is_absolute() {
                return Err(AppError::new("invalid_file", "请选择本机文件。"));
            }
            core::validate_upload_file(&path)?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| AppError::new("invalid_file", "文件名无法表示为 UTF-8。"))?
                .to_string();
            core::upload_filename(&name)?;
            return Ok(PreparedUpload {
                path,
                name,
                _temporary: None,
            });
        }
        UploadSource::FileData { name, bytes } => {
            core::upload_filename(&name)?;
            let mut temporary = tempfile::NamedTempFile::new()
                .map_err(|e| error("temporary_file", "无法创建临时文件。", e))?;
            std::io::Write::write_all(&mut temporary, &bytes)
                .map_err(|e| error("file_write", "无法准备粘贴文件。", e))?;
            return Ok(PreparedUpload {
                path: temporary.path().to_path_buf(),
                name,
                _temporary: Some(temporary),
            });
        }
        UploadSource::Bytes(bytes) => decode_image(bytes)?,
    };
    let temporary = tempfile::Builder::new()
        .prefix("termius-image-")
        .suffix(".png")
        .tempfile()
        .map_err(|e| error("temporary_file", "无法创建临时图片。", e))?;
    image
        .save_with_format(temporary.path(), image::ImageFormat::Png)
        .map_err(|e| error("image_encode", "无法准备 PNG 图片。", e))?;
    Ok(PreparedUpload {
        path: temporary.path().to_path_buf(),
        name: "image.png".into(),
        _temporary: Some(temporary),
    })
}
fn decode_image(bytes: Vec<u8>) -> AppResult<image::DynamicImage> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(AppError::new("image_too_large", "图片不能超过 32 MiB。"));
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| error("invalid_image", "无法识别图片格式。", e))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| error("invalid_image", "图片损坏、格式不支持或尺寸过大。", e))
}

#[tauri::command]
fn check_update(app: AppHandle) -> AppResult<()> {
    updater::check(app)
}
#[tauri::command]
fn restart_for_update(app: AppHandle) -> AppResult<()> {
    let s = state(&app);
    let busy = s.busy.lock().unwrap();
    if busy.is_some() {
        return Err(AppError::new(
            "operation_busy",
            "请等待当前操作完成后再重启。",
        ));
    }
    if s.update.lock().unwrap().state != "ready" {
        return Err(AppError::new("update_not_ready", "新版本尚未准备好。"));
    }
    s.closing.store(true, Ordering::SeqCst);
    match updater::schedule(true) {
        Ok(true) => {
            drop(busy);
            app.exit(0);
            Ok(())
        }
        result => {
            s.closing.store(false, Ordering::SeqCst);
            result.and_then(|_| Err(AppError::new("update_not_ready", "没有可应用的更新。")))
        }
    }
}
fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
fn quit_app(app: &AppHandle) {
    let s = state(app);
    s.closing.store(true, Ordering::SeqCst);
    if let Some(job) = s.busy.lock().unwrap().as_ref() {
        job.token.cancel();
    }
    let _ = updater::schedule(false);
    app.exit(0);
}
async fn linux_tray_host() -> bool {
    let mut command = tokio::process::Command::new("gdbus");
    command
        .args([
            "call",
            "--session",
            "--dest",
            "org.kde.StatusNotifierWatcher",
            "--object-path",
            "/StatusNotifierWatcher",
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            "org.kde.StatusNotifierWatcher",
            "IsStatusNotifierHostRegistered",
        ])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    matches!(tokio::time::timeout(Duration::from_secs(2),command.output()).await,Ok(Ok(output))if output.status.success()&&String::from_utf8_lossy(&output.stdout).trim()=="(<true>,)")
}
fn observe_target(app: &AppHandle) {
    if !platform::capabilities().automatic_target_selection {
        return;
    }
    let Some(target) = platform::capture_target() else {
        return;
    };
    let connection = termius::capture(target.process);
    let s = state(app);
    let settings = s.settings.lock().unwrap();
    let detected = match connection {
        Ok(connection) => match core::server_for_connection(&settings.servers, &connection) {
            Ok(server) => DetectedTarget {
                session: connection.endpoint(),
                server_id: Some(server.id.clone()),
                message: format!("已识别当前真实连接：{}", server.name),
            },
            Err(error) => DetectedTarget {
                session: connection.endpoint(),
                server_id: None,
                message: error.message,
            },
        },
        Err(error) => DetectedTarget {
            session: String::new(),
            server_id: None,
            message: error.message,
        },
    };
    drop(settings);
    let mut runtime = s.runtime.lock().unwrap();
    if runtime.detected_target.as_ref() != Some(&detected) {
        runtime.detected_target = Some(detected);
        drop(runtime);
        publish(app);
    }
}
fn bind_shortcuts(app: &AppHandle) {
    observe_target(app);
    let s = state(app);
    let settings = s.settings.lock().unwrap().clone();
    let caps = platform::capabilities();
    let reason = if s.startup_error.is_some() {
        Some("配置文件读取失败，原文件已保留")
    } else if !settings.enabled {
        Some("已暂停")
    } else if !caps.hotkey_available {
        Some("当前桌面使用手动上传")
    } else if if caps.automatic_target_selection {
        !settings.servers.iter().any(|p| p.validate(true).is_ok())
    } else {
        settings
            .active_server()
            .is_none_or(|p| p.validate(true).is_err())
    } {
        Some("请先配置并选择服务器")
    } else if s.busy.lock().unwrap().is_some() {
        Some("操作进行中")
    } else if platform::capture_target().is_none() {
        Some("等待 Termius 位于前台")
    } else {
        None
    };
    let mut bindings = s.bindings.lock().unwrap();
    let mut desired = Vec::new();
    let mut status = if let Some(reason) = reason {
        reason.to_string()
    } else {
        match parse_shortcut(&settings.shortcut) {
            Ok(key) => {
                desired.push(key);
                "快捷键可用".into()
            }
            Err(e) => e.message,
        }
    };
    if !desired.is_empty() && settings.legacy_shortcut_enabled {
        if let Ok(key) = parse_shortcut("Ctrl+Alt+V") {
            if !desired.contains(&key) {
                desired.push(key);
            }
        }
    }
    if bindings.dirty || bindings.requested != desired {
        let _ = app.global_shortcut().unregister_all();
        bindings.registered.clear();
        for (i, key) in desired.iter().enumerate() {
            match app.global_shortcut().register(*key) {
                Ok(()) => bindings.registered.push(*key),
                Err(e) => {
                    status = if i == 0 {
                        format!("主快捷键不可用：{e}")
                    } else {
                        "主快捷键可用，备用快捷键被占用".into()
                    };
                    if i == 0 {
                        break;
                    }
                }
            }
        }
        bindings.requested = desired.clone();
        bindings.status = status.clone();
        bindings.dirty = false;
    } else if !desired.is_empty() {
        status = bindings.status.clone();
    }
    drop(bindings);
    let mut runtime = s.runtime.lock().unwrap();
    if runtime.shortcut_status != status {
        runtime.shortcut_status = status;
        drop(runtime);
        publish(app);
    }
}
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_window(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init());
    if !platform::is_wayland() {
        builder = builder.plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state == ShortcutState::Pressed {
                        if let Some(target) = platform::capture_target() {
                            if let Err(e) =
                                start_upload(app.clone(), UploadSource::Clipboard, Some(target))
                            {
                                let s = state(app);
                                if s.settings.lock().unwrap().notifications {
                                    let handle = app.clone();
                                    let message = e.message.clone();
                                    tauri::async_runtime::spawn_blocking(move || {
                                        let _ = handle
                                            .notification()
                                            .builder()
                                            .title("Termius 贴图助手")
                                            .body(message)
                                            .show();
                                    });
                                }
                                s.runtime.lock().unwrap().last_result = Some(LastResult {
                                    status: "error".into(),
                                    stage: "start".into(),
                                    message: e.message,
                                    path: None,
                                    target: None,
                                    at: now(),
                                    details: e.detail,
                                });
                                publish(app);
                            }
                        }
                    }
                })
                .build(),
        );
    }
    builder
        .invoke_handler(tauri::generate_handler![
            get_state,
            check_update,
            restart_for_update,
            save_server,
            remove_server,
            set_active_server,
            set_enabled,
            save_preferences,
            scan_host_key,
            login_server,
            browse_remote_directory,
            cancel_operation,
            choose_key_file,
            choose_upload_file,
            copy_path,
            upload_clipboard,
            upload_file,
            upload_image_data,
            upload_file_data
        ])
        .setup(|app| {
            let directory = app.path().app_config_dir()?;
            let store = SettingsStore::new(directory.clone());
            let (settings, startup_error) = match store.load() {
                Ok(loaded) => (loaded.settings, None),
                Err(e) => (Settings::default(), Some(e)),
            };
            let runtime = RuntimeState {
                shortcut_status: "正在检查系统能力…".into(),
                operation: None,
                detected_target: None,
                last_result: startup_error.as_ref().map(|e| LastResult {
                    status: "error".into(),
                    stage: "settings".into(),
                    message: e.message.clone(),
                    path: None,
                    target: None,
                    at: now(),
                    details: e.detail.clone(),
                }),
            };
            app.manage(Arc::new(AppState {
                settings: Mutex::new(settings),
                mutation: Mutex::new(()),
                store,
                ssh: SshClient::new(directory),
                runtime: Mutex::new(runtime),
                busy: Mutex::new(None),
                bindings: Mutex::new(Hotkeys::default()),
                generation: AtomicU64::new(0),
                operation_id: AtomicU64::new(1),
                closing: AtomicBool::new(false),
                tray_available: AtomicBool::new(false),
                startup_error,
                update: Mutex::new(updater::initial()),
                update_checking: AtomicBool::new(false),
            }));
            updater::start(app.handle().clone());
            platform::start_watcher();
            let show =
                tauri::menu::MenuItem::with_id(app, "show", "打开贴图助手", true, None::<&str>)?;
            let pause = tauri::menu::MenuItem::with_id(
                app,
                "pause",
                "暂停 / 继续贴图",
                true,
                None::<&str>,
            )?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出软件", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &pause, &quit])?;
            let tray = tauri::tray::TrayIconBuilder::new()
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/icon.png"
                ))?)
                .tooltip("Termius 贴图助手")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_window(app),
                    "pause" => {
                        let enabled = !state(app).settings.lock().unwrap().enabled;
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = set_enabled(handle, enabled).await;
                        });
                    }
                    "quit" => quit_app(app),
                    _ => {}
                })
                .build(app);
            // On Linux a successfully-created icon does not prove a visible tray host.
            // Keep the window reachable in the taskbar when no host is established.
            state(app.handle())
                .tray_available
                .store(tray.is_ok() && cfg!(windows), Ordering::SeqCst);
            if cfg!(target_os = "linux") && tray.is_ok() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        let visible = linux_tray_host().await;
                        let previous = state(&handle)
                            .tray_available
                            .swap(visible, Ordering::SeqCst);
                        if visible != previous {
                            if previous && !visible {
                                if let Some(w) = handle.get_webview_window("main") {
                                    let _ = w.show();
                                }
                            }
                            publish(&handle);
                        }
                        tokio::time::sleep(Duration::from_secs(15)).await;
                    }
                });
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    bind_shortcuts(&handle);
                    tokio::time::sleep(Duration::from_millis(120)).await;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let s = state(window.app_handle());
                if !s.closing.load(Ordering::SeqCst) {
                    if s.settings.lock().unwrap().close_to_tray {
                        api.prevent_close();
                        if s.tray_available.load(Ordering::SeqCst) {
                            let _ = window.hide();
                        } else {
                            let _ = window.minimize();
                        }
                    } else {
                        // A tray icon can keep Tauri alive after its last window
                        // is destroyed. Explicitly exit while the window still
                        // exists, so disabling background mode cannot strand it.
                        api.prevent_close();
                        quit_app(window.app_handle());
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("无法启动贴图助手")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let _ = updater::schedule(false);
                if let Some(s) = app.try_state::<Arc<AppState>>() {
                    if let Some(job) = s.busy.lock().unwrap().as_ref() {
                        job.token.cancel();
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_path_and_special_path() {
        assert_eq!(prompt_path("/home/a/image.png"), "/home/a/image.png ");
        assert_eq!(prompt_path("/tmp/a b'中.png"), "'/tmp/a b'\\''中.png' ");
    }
    #[test]
    fn invalid_image_is_rejected() {
        assert!(decode_image(b"not an image".to_vec()).is_err());
    }
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "显式启用后仅扫描并验证旧版当前服务器，不上传或授权"]
    async fn live_existing_server_readonly() {
        assert_eq!(std::env::var("TERMIUS_LIVE_CHECK").as_deref(), Ok("1"));
        let scratch = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(scratch.path().join("settings"));
        let loaded = store.load().unwrap();
        let profile = loaded
            .settings
            .active_server()
            .expect("旧版须有已选择的服务器");
        profile.validate(true).unwrap();
        let client = SshClient::new(scratch.path().join("connection"));
        let candidate = client
            .scan_host_key(profile, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            Some(&candidate),
            profile.host_key.as_ref(),
            "实测指纹必须与已有可信记录相同"
        );
        client
            .test(profile, CancellationToken::new())
            .await
            .unwrap();
        assert!(
            !store.settings_path().exists(),
            "只读验证不能保存或改变服务器信任"
        );
    }
}
