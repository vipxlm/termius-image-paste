use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::IpAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tempfile::NamedTempFile;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::Command,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
}

impl AppError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    fn because(code: &str, message: &str, detail: impl ToString) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: Some(detail.to_string()),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for AppError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub key_file: String,
    pub remote_directory: String,
    #[serde(default)]
    pub host_key: Option<String>,
    #[serde(default)]
    pub verified_at: Option<String>,
}

impl Default for ServerProfile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4().simple().to_string(),
            name: "新服务器".into(),
            host: String::new(),
            port: 22,
            username: String::new(),
            key_file: String::new(),
            remote_directory: String::new(),
            host_key: None,
            verified_at: None,
        }
    }
}

impl ServerProfile {
    pub fn validate(&self, require_trust: bool) -> AppResult<()> {
        if !valid_id(&self.id) {
            return Err(AppError::new("invalid_id", "服务器标识无效。"));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 128 || has_control(&self.name)
        {
            return Err(AppError::new("invalid_name", "请填写有效的服务器名称。"));
        }
        if !valid_host(&self.host) {
            return Err(AppError::new(
                "invalid_host",
                "服务器地址须为 DNS 名称、IPv4 或不带方括号的 IPv6 地址。",
            ));
        }
        let mut username = self
            .username
            .strip_suffix('$')
            .unwrap_or(&self.username)
            .chars();
        if self.username.len() > 64
            || !username
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            || !username.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        {
            return Err(AppError::new("invalid_username", "SSH 用户名无效。"));
        }
        if self.port == 0 {
            return Err(AppError::new("invalid_port", "端口须在 1 到 65535 之间。"));
        }
        if !self.remote_directory.starts_with('/')
            || self.remote_directory.len() > 4096
            || has_control(&self.remote_directory)
        {
            return Err(AppError::new(
                "invalid_remote_directory",
                "远程图片目录须为不含控制字符的 Linux 绝对路径。",
            ));
        }
        let key = Path::new(&self.key_file);
        if !key.is_absolute() || key.file_name().is_none() || has_control(&self.key_file) {
            return Err(AppError::new(
                "invalid_key_path",
                "请选择专用 SSH 密钥的本机绝对路径。",
            ));
        }
        match self.host_key.as_deref() {
            Some(key) if !key.is_empty() => {
                decode_host_key(key)?;
            }
            _ if require_trust => {
                return Err(AppError::new(
                    "untrusted_host",
                    "请先核对并信任此服务器的 SSH 主机指纹。",
                ))
            }
            _ => {}
        }
        if let Some(value) = self.verified_at.as_deref() {
            chrono::DateTime::parse_from_rfc3339(value)
                .map_err(|_| AppError::new("invalid_verified_at", "主机指纹确认时间无效。"))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub schema_version: u32,
    pub servers: Vec<ServerProfile>,
    pub active_server_id: Option<String>,
    pub shortcut: String,
    pub legacy_shortcut_enabled: bool,
    pub enabled: bool,
    pub close_to_tray: bool,
    pub notifications: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 2,
            servers: Vec::new(),
            active_server_id: None,
            shortcut: "Ctrl+Shift+F12".into(),
            legacy_shortcut_enabled: false,
            enabled: true,
            close_to_tray: true,
            notifications: true,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> AppResult<()> {
        if self.schema_version != 2 {
            return Err(AppError::new(
                "unsupported_settings",
                "配置版本不受支持，原文件已保留。",
            ));
        }
        if self.servers.len() > 100 {
            return Err(AppError::new(
                "too_many_servers",
                "最多可配置 100 台服务器。",
            ));
        }
        let mut ids = HashSet::new();
        for server in &self.servers {
            server.validate(false)?;
            if !ids.insert(&server.id) {
                return Err(AppError::new("duplicate_server", "服务器标识重复。"));
            }
        }
        if let Some(id) = &self.active_server_id {
            if !self.servers.iter().any(|server| &server.id == id) {
                return Err(AppError::new(
                    "invalid_active_server",
                    "当前上传目标不存在，请重新选择。",
                ));
            }
        }
        validate_shortcut(&self.shortcut)
    }

    pub fn active_server(&self) -> Option<&ServerProfile> {
        self.active_server_id
            .as_ref()
            .and_then(|id| self.servers.iter().find(|server| &server.id == id))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedSettings {
    pub settings: Settings,
    pub migrated: bool,
}

pub struct SettingsStore {
    config_dir: PathBuf,
    legacy_path: Option<PathBuf>,
}

impl SettingsStore {
    pub fn new(config_dir: PathBuf) -> Self {
        #[cfg(windows)]
        let legacy_path = std::env::var_os("LOCALAPPDATA").map(|local| {
            PathBuf::from(local)
                .join("TermiusImagePaste")
                .join("settings.json")
        });
        #[cfg(not(windows))]
        let legacy_path = None;
        Self {
            config_dir,
            legacy_path,
        }
    }
    pub fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    pub fn load(&self) -> AppResult<LoadedSettings> {
        let path = self.settings_path();
        if path.exists() {
            let settings: Settings = read_json(&path)?;
            settings.validate()?;
            return Ok(LoadedSettings {
                settings,
                migrated: false,
            });
        }
        if let Some(legacy) = &self.legacy_path {
            if legacy != &path && legacy.exists() {
                let settings = migrate_legacy(read_json(legacy)?)?;
                return Ok(LoadedSettings {
                    settings,
                    migrated: true,
                });
            }
        }
        Ok(LoadedSettings {
            settings: Settings::default(),
            migrated: false,
        })
    }

    pub fn save(&self, mut settings: Settings) -> AppResult<Settings> {
        settings.validate()?;
        let previous = self.load()?.settings;
        for server in &mut settings.servers {
            if let Some(old) = previous.servers.iter().find(|old| old.id == server.id) {
                if (old.host != server.host || old.port != server.port)
                    && old.host_key == server.host_key
                    && old.verified_at == server.verified_at
                {
                    server.host_key = None;
                    server.verified_at = None;
                }
            }
            if server.host_key.as_deref().is_none_or(str::is_empty) {
                server.host_key = None;
                server.verified_at = None;
            }
        }
        create_private_directory(&self.config_dir)?;
        let mut file = NamedTempFile::new_in(&self.config_dir)
            .map_err(|e| AppError::because("settings_write", "无法创建配置临时文件。", e))?;
        serde_json::to_writer_pretty(file.as_file_mut(), &settings)
            .map_err(|e| AppError::because("settings_write", "无法序列化配置。", e))?;
        file.write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|e| AppError::because("settings_write", "无法写入配置，原文件已保留。", e))?;
        file.persist(self.settings_path()).map_err(|e| {
            AppError::because(
                "settings_write",
                "无法原子保存配置，原文件已保留。",
                e.error,
            )
        })?;
        Ok(settings)
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> AppResult<T> {
    let metadata = fs::metadata(path)
        .map_err(|e| AppError::because("settings_read", "无法读取配置，原文件已保留。", e))?;
    if metadata.len() > 1024 * 1024 {
        return Err(AppError::new(
            "settings_read",
            "配置文件过大，原文件已保留。",
        ));
    }
    let content = fs::read_to_string(path)
        .map_err(|e| AppError::because("settings_read", "无法读取配置，原文件已保留。", e))?;
    serde_json::from_str(content.trim_start_matches('\u{feff}'))
        .map_err(|e| AppError::because("settings_read", "配置文件损坏，原文件已保留。", e))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LegacySettings {
    servers: Vec<LegacyServer>,
    active_server_id: String,
    #[serde(default = "legacy_modifiers")]
    hotkey_modifiers: u32,
    #[serde(default = "legacy_key")]
    hotkey_key: u32,
    #[serde(default = "yes")]
    legacy_hotkey_enabled: bool,
    #[serde(default = "yes")]
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LegacyServer {
    id: String,
    name: String,
    host: String,
    port: u16,
    #[serde(rename = "User", alias = "Username")]
    username: String,
    key_file: String,
    remote_directory: String,
    #[serde(default)]
    host_key: Option<String>,
}
fn legacy_modifiers() -> u32 {
    6
}
fn legacy_key() -> u32 {
    123
}
fn yes() -> bool {
    true
}

fn migrate_legacy(old: LegacySettings) -> AppResult<Settings> {
    let settings = Settings {
        servers: old
            .servers
            .into_iter()
            .map(|s| ServerProfile {
                id: s.id,
                name: s.name,
                host: s.host,
                port: s.port,
                username: s.username,
                key_file: s.key_file,
                remote_directory: s.remote_directory,
                host_key: s.host_key.filter(|key| !key.is_empty()),
                verified_at: None,
            })
            .collect(),
        active_server_id: Some(old.active_server_id),
        shortcut: legacy_shortcut(old.hotkey_modifiers, old.hotkey_key)?,
        legacy_shortcut_enabled: old.legacy_hotkey_enabled,
        enabled: old.enabled,
        ..Settings::default()
    };
    settings.validate()?;
    Ok(settings)
}

fn legacy_shortcut(modifiers: u32, key: u32) -> AppResult<String> {
    if modifiers == 0 || modifiers & !7 != 0 {
        return Err(AppError::new(
            "invalid_shortcut",
            "旧版快捷键无效，原配置已保留。",
        ));
    }
    let key = match key {
        0x30..=0x39 | 0x41..=0x5a => char::from_u32(key).unwrap().to_string(),
        0x70..=0x87 => format!("F{}", key - 0x6f),
        _ => {
            return Err(AppError::new(
                "invalid_shortcut",
                "旧版快捷键主键无效，原配置已保留。",
            ))
        }
    };
    let mut parts = Vec::new();
    if modifiers & 2 != 0 {
        parts.push("Ctrl".to_string());
    }
    if modifiers & 1 != 0 {
        parts.push("Alt".to_string());
    }
    if modifiers & 4 != 0 {
        parts.push("Shift".to_string());
    }
    parts.push(key);
    Ok(parts.join("+"))
}

pub fn validate_shortcut(value: &str) -> AppResult<()> {
    let mut parts: Vec<_> = value.split('+').collect();
    let key = parts.pop().unwrap_or("");
    let valid_key = (key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric())
        || key
            .strip_prefix('F')
            .and_then(|v| v.parse::<u8>().ok())
            .is_some_and(|v| (1..=24).contains(&v));
    let mut seen = HashSet::new();
    if !valid_key
        || parts.is_empty()
        || parts
            .iter()
            .any(|part| !matches!(*part, "Ctrl" | "Alt" | "Shift" | "Super") || !seen.insert(*part))
    {
        return Err(AppError::new(
            "invalid_shortcut",
            "快捷键须包含 Ctrl、Alt、Shift 或 Super，主键使用字母、数字或 F1–F24。",
        ));
    }
    Ok(())
}

fn has_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}
fn valid_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}
fn valid_host(host: &str) -> bool {
    if host.is_empty()
        || host.len() > 253
        || has_control(host)
        || host.chars().any(char::is_whitespace)
        || host.starts_with('-')
        || host.contains(['[', ']', '/', '\\', '@', '\'', '"'])
    {
        return false;
    }
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    host.strip_suffix('.')
        .unwrap_or(host)
        .split('.')
        .all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

fn decode_host_key(value: &str) -> AppResult<Vec<u8>> {
    let mut parts = value.split(' ');
    if parts.next() != Some("ssh-ed25519") {
        return Err(AppError::new("invalid_host_key", "主机公钥须为 ED25519。"));
    }
    let encoded = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(AppError::new("invalid_host_key", "主机公钥格式无效。"));
    }
    let blob = STANDARD
        .decode(encoded)
        .map_err(|e| AppError::because("invalid_host_key", "主机公钥编码无效。", e))?;
    if blob.len() != 51
        || blob[..4] != [0, 0, 0, 11]
        || &blob[4..15] != b"ssh-ed25519"
        || blob[15..19] != [0, 0, 0, 32]
    {
        return Err(AppError::new(
            "invalid_host_key",
            "ED25519 主机公钥结构或长度无效。",
        ));
    }
    Ok(blob)
}

pub fn fingerprint(host_key: &str) -> AppResult<String> {
    Ok(format!(
        "SHA256:{}",
        STANDARD
            .encode(Sha256::digest(decode_host_key(host_key)?))
            .trim_end_matches('=')
    ))
}

pub fn quote_posix(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn create_private_directory(path: &Path) -> AppResult<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|e| AppError::because("local_directory", "无法创建本机工作目录。", e))
}

pub struct SshClient {
    state_dir: PathBuf,
}

impl SshClient {
    pub fn new(state_dir: PathBuf) -> Self {
        Self { state_dir }
    }

    pub async fn scan_host_key(
        &self,
        profile: &ServerProfile,
        token: CancellationToken,
    ) -> AppResult<String> {
        // Trust discovery needs only an endpoint; it must also work before key generation.
        if !valid_host(&profile.host) || profile.port == 0 {
            return Err(AppError::new(
                "invalid_host",
                "请填写有效的服务器地址和 SSH 端口。",
            ));
        }
        let scratch = NamedTempFile::new()
            .map_err(|e| AppError::because("scan_failed", "无法创建临时主机信任文件。", e))?
            .into_temp_path();
        let mut args = strings(&["-F", "none", "-N", "-T", "-p", &profile.port.to_string()]);
        for option in [
            "ConnectTimeout=10",
            "BatchMode=yes",
            "StrictHostKeyChecking=accept-new",
            "GlobalKnownHostsFile=none",
            "HashKnownHosts=no",
            "HostKeyAlgorithms=ssh-ed25519",
            "PreferredAuthentications=none",
            "PubkeyAuthentication=no",
            "PasswordAuthentication=no",
            "KbdInteractiveAuthentication=no",
            "HostbasedAuthentication=no",
            "GSSAPIAuthentication=no",
            "IdentityAgent=none",
            "IdentityFile=none",
            "ClearAllForwardings=yes",
            "PermitLocalCommand=no",
            "VerifyHostKeyDNS=no",
            "UpdateHostKeys=no",
        ] {
            option_arg(&mut args, option);
        }
        option_arg(
            &mut args,
            &format!("UserKnownHostsFile={}", config_path(&scratch)?),
        );
        args.push(scan_target(profile));
        run_program("ssh", args, &[], Duration::from_secs(30), token, &[0, 255]).await?;
        let content = fs::read_to_string(&scratch)
            .map_err(|e| AppError::because("scan_failed", "无法读取临时主机公钥。", e))?;
        parse_scanned_key(&content)
    }

    pub async fn test(&self, profile: &ServerProfile, token: CancellationToken) -> AppResult<()> {
        profile.validate(true)?;
        require_key(profile)?;
        let mut args = self.base_args(profile, true)?;
        let directory = quote_posix(&profile.remote_directory);
        args.extend([
            "-p".into(),
            profile.port.to_string(),
            target(profile),
            format!("test -d {directory} && test -w {directory} && printf '%s' 'ready'"),
        ]);
        let output = run_program("ssh", args, &[], Duration::from_secs(30), token, &[0]).await?;
        if output.stdout != "ready" {
            return Err(AppError::new(
                "directory_check",
                "密钥登录或远程目录可写性验证失败。",
            ));
        }
        Ok(())
    }

    pub async fn remote_directory(
        &self,
        profile: &ServerProfile,
        directory: Option<&str>,
        browse: bool,
        token: CancellationToken,
    ) -> AppResult<Vec<String>> {
        profile.validate(true)?;
        require_key(profile)?;
        let command = if let Some(path) = directory {
            if !path.starts_with('/') || has_control(path) || path.len() > 4096 {
                return Err(AppError::new(
                    "invalid_directory",
                    "请选择有效的远端绝对路径。",
                ));
            }
            format!("cd -- {} || exit 1; ", quote_posix(path))
        } else if browse {
            "cd -- \"$HOME\" || exit 1; ".into()
        } else {
            "umask 077; mkdir -p -- \"$HOME/.cache/termius-images\" && cd -- \"$HOME/.cache/termius-images\" || exit 1; ".into()
        };
        let listing = if browse {
            "printf '%s\\0' \"$PWD\"; for entry in \"$PWD\"/*/ \"$PWD\"/.[!.]*/ \"$PWD\"/..?*/; do [ -d \"$entry\" ] && printf '%s\\0' \"${entry%/}\"; done; true"
        } else {
            "test -w . || exit 1; printf '%s\\0' \"$PWD\""
        };
        let mut args = self.base_args(profile, true)?;
        args.extend([
            "-p".into(),
            profile.port.to_string(),
            target(profile),
            command + listing,
        ]);
        let output = run_program("ssh", args, &[], Duration::from_secs(30), token, &[0]).await?;
        parse_remote_directories(&output.stdout)
    }

    pub async fn generate_key(
        &self,
        profile: &ServerProfile,
        token: CancellationToken,
    ) -> AppResult<String> {
        let mut selected = profile.clone();
        if selected.key_file.is_empty() {
            if !valid_id(&selected.id) {
                return Err(AppError::new("invalid_id", "服务器标识无效。"));
            }
            selected.key_file = self
                .state_dir
                .join("keys")
                .join(format!("{}_ed25519", selected.id))
                .to_string_lossy()
                .into_owned();
        }
        selected.validate(false)?;
        if token.is_cancelled() {
            return Err(cancelled());
        }
        let path = Path::new(&selected.key_file);
        if path.exists() || public_path(path).exists() {
            return Err(AppError::new(
                "key_exists",
                "此路径已有私钥或公钥，请选择其他路径；原文件不会覆盖。",
            ));
        }
        create_private_directory(
            path.parent()
                .ok_or_else(|| AppError::new("invalid_key_path", "专用密钥路径无效。"))?,
        )?;
        run_program(
            "ssh-keygen",
            strings(&[
                "-q",
                "-t",
                "ed25519",
                "-f",
                &selected.key_file,
                "-N",
                "",
                "-C",
                "termius-image-paste",
            ]),
            &[],
            Duration::from_secs(30),
            token,
            &[0],
        )
        .await?;
        if !path.is_file() || !public_path(path).is_file() {
            return Err(AppError::new(
                "key_generation",
                "未能生成完整的专用密钥对。",
            ));
        }
        Ok(selected.key_file)
    }

    pub async fn upload(
        &self,
        profile: &ServerProfile,
        local_path: &Path,
        token: CancellationToken,
    ) -> AppResult<String> {
        profile.validate(true)?;
        require_key(profile)?;
        validate_png(local_path)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let name = format!(
            "image-{}-{}.png",
            Utc::now().format("%Y%m%d-%H%M%S"),
            Uuid::new_v4().simple()
        );
        let remote = format!(
            "{}/{}",
            profile.remote_directory.trim_end_matches('/'),
            name
        );
        let mut prepare = self.base_args(profile, true)?;
        let directory = quote_posix(&profile.remote_directory);
        prepare.extend([
            "-p".into(),
            profile.port.to_string(),
            target(profile),
            format!(
                "umask 077; mkdir -p -- {directory} && test -d {directory} && test -w {directory}"
            ),
        ]);
        run_program(
            "ssh",
            prepare,
            &[],
            Duration::from_secs(30),
            token.clone(),
            &[0],
        )
        .await?;
        let mut args = self.base_args(profile, true)?;
        let local = fs::canonicalize(local_path)
            .map_err(|e| AppError::because("invalid_image", "无法打开待上传图片。", e))?;
        args.extend(scp_upload_arguments(profile, &local, &remote)?);
        // Force SFTP on clients which predate its becoming the default. Clients
        // too old to support -s fail instead of passing a literal path to a shell.
        run_program(
            "scp",
            args,
            &[],
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            token,
            &[0],
        )
        .await?;
        Ok(remote)
    }

    pub async fn authorize(
        &self,
        profile: &ServerProfile,
        password: String,
        askpass_exe: &Path,
        token: CancellationToken,
    ) -> AppResult<()> {
        let password = Zeroizing::new(password);
        profile.validate(true)?;
        require_key(profile)?;
        if !askpass_exe.is_file() {
            return Err(AppError::new(
                "askpass_missing",
                "找不到本次授权所需的密码助手。",
            ));
        }
        if password.is_empty() || password.len() > 8190 || password.contains(['\r', '\n', '\0']) {
            return Err(AppError::new(
                "invalid_password",
                "请输入有效的单行登录密码。",
            ));
        }
        let key = self.authorization_key(profile, token.clone()).await?;
        let entry = quote_posix(&format!("restrict {key}"));
        let directory = quote_posix(&profile.remote_directory);
        let remote_command = format!("umask 077; mkdir -p -- \"$HOME/.ssh\" {directory} && touch \"$HOME/.ssh/authorized_keys\" && (grep -qxF -- {entry} \"$HOME/.ssh/authorized_keys\" || printf '\\n%s\\n' {entry} >> \"$HOME/.ssh/authorized_keys\")");
        let mut args = self.base_args(profile, false)?;
        for option in [
            "BatchMode=no",
            "PubkeyAuthentication=no",
            "PreferredAuthentications=password,keyboard-interactive",
            "NumberOfPasswordPrompts=1",
        ] {
            option_arg(&mut args, option);
        }
        args.extend([
            "-p".into(),
            profile.port.to_string(),
            target(profile),
            remote_command,
        ]);
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| {
                AppError::because("askpass_listener", "无法建立本次授权的本机密码通道。", e)
            })?;
        let address = listener
            .local_addr()
            .map_err(|e| AppError::because("askpass_listener", "无法读取本机密码通道。", e))?;
        let secret_token = Uuid::new_v4().simple().to_string();
        let server = PasswordServer::start(listener, secret_token.clone(), password, token.clone());
        let environment = vec![
            (
                "SSH_ASKPASS".into(),
                askpass_exe.to_string_lossy().into_owned(),
            ),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            ("DISPLAY".into(), "termius-image-paste".into()),
            ("TERMIUS_ASKPASS_ADDR".into(), address.to_string()),
            ("TERMIUS_ASKPASS_TOKEN".into(), secret_token),
        ];
        let result = run_program(
            "ssh",
            args,
            &environment,
            Duration::from_secs(120),
            token.clone(),
            &[0],
        )
        .await;
        server.stop().await;
        if let Err(error) = result {
            if matches!(error.code.as_str(), "cancelled" | "timeout") {
                return Err(error);
            }
            return Err(AppError {
                code: "authorization_failed".into(),
                message:
                    "授权失败。请核对密码；首次授权仅支持单次密码认证，不支持 MFA 或交互问答。"
                        .into(),
                detail: error.detail,
            });
        }
        Ok(())
    }

    async fn authorization_key(
        &self,
        profile: &ServerProfile,
        token: CancellationToken,
    ) -> AppResult<String> {
        let output = run_program(
            "ssh-keygen",
            strings(&["-y", "-f", &profile.key_file, "-P", ""]),
            &[("SSH_ASKPASS_REQUIRE".into(), "never".into())],
            Duration::from_secs(30),
            token,
            &[0],
        )
        .await
        .map_err(|error| {
            if matches!(error.code.as_str(), "cancelled" | "timeout") {
                error
            } else {
                AppError {
                    code: "key_requires_interaction".into(),
                    message: "首次授权需要可免交互读取的 SSH 私钥；请使用专用无口令密钥。".into(),
                    detail: error.detail,
                }
            }
        })?;
        let key = first_public_key(&output.stdout)?;
        if !key.starts_with("ssh-ed25519 ") {
            return Err(AppError::new(
                "unsupported_client_key",
                "首次授权目前使用 ED25519 专用密钥，其他类型可预先授权后选择使用。",
            ));
        }
        decode_host_key(&key).map_err(|_| {
            AppError::new("invalid_client_key", "所选私钥派生的公钥无效，已停止授权。")
        })?;
        let public = public_path(Path::new(&profile.key_file));
        if public.exists() {
            let content = fs::read_to_string(public).map_err(|e| {
                AppError::because("public_key_read", "无法读取对应公钥，已停止授权。", e)
            })?;
            if first_public_key(&content)? != key {
                return Err(AppError::new(
                    "keypair_mismatch",
                    "所选私钥与 .pub 公钥不匹配，已停止授权并保留原文件。",
                ));
            }
        }
        Ok(key)
    }

    fn base_args(&self, profile: &ServerProfile, batch: bool) -> AppResult<Vec<String>> {
        let known_hosts = self.known_hosts(profile)?;
        let mut args = strings(&["-F", "none"]);
        for option in [
            "ConnectTimeout=10",
            "StrictHostKeyChecking=yes",
            "GlobalKnownHostsFile=none",
            "HostKeyAlgorithms=ssh-ed25519",
            "IdentitiesOnly=yes",
            "ServerAliveInterval=5",
            "ServerAliveCountMax=2",
            "LogLevel=ERROR",
            "ClearAllForwardings=yes",
            "PermitLocalCommand=no",
            "VerifyHostKeyDNS=no",
            "UpdateHostKeys=no",
        ] {
            option_arg(&mut args, option);
        }
        option_arg(
            &mut args,
            &format!("UserKnownHostsFile={}", config_path(&known_hosts)?),
        );
        if batch {
            option_arg(&mut args, "BatchMode=yes");
            args.extend(["-i".into(), profile.key_file.clone()]);
        }
        Ok(args)
    }

    fn known_hosts(&self, profile: &ServerProfile) -> AppResult<PathBuf> {
        profile.validate(true)?;
        let key = profile.host_key.as_deref().unwrap();
        let identity = format!(
            "{}\n{}\n{}",
            profile.host.to_ascii_lowercase(),
            profile.port,
            key
        );
        let hash = Sha256::digest(identity.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        create_private_directory(&self.state_dir)?;
        let path = self.state_dir.join(format!("known_hosts-{hash}"));
        let host = if profile.port == 22 {
            profile.host.to_ascii_lowercase()
        } else {
            format!("[{}]:{}", profile.host.to_ascii_lowercase(), profile.port)
        };
        let content = format!("{host} {key}\n");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => file
                .write_all(content.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|e| {
                    AppError::because("known_hosts_write", "无法写入隔离的主机信任文件。", e)
                })?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read_to_string(&path).ok().as_deref() != Some(content.as_str()) {
                    return Err(AppError::new(
                        "known_hosts_conflict",
                        "隔离的主机信任文件不一致，已停止连接并保留原文件。",
                    ));
                }
            }
            Err(error) => {
                return Err(AppError::because(
                    "known_hosts_write",
                    "无法创建隔离的主机信任文件。",
                    error,
                ))
            }
        }
        Ok(path)
    }
}

fn parse_remote_directories(output: &str) -> AppResult<Vec<String>> {
    let paths: Vec<String> = output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect();
    if !output.ends_with('\0')
        || paths.is_empty()
        || paths
            .iter()
            .any(|path| !path.starts_with('/') || has_control(path))
    {
        return Err(AppError::new(
            "directory_output",
            "无法读取远端目录，请检查服务器登录脚本输出。",
        ));
    }
    Ok(paths)
}

/// Match the endpoint reported by Termius; display names are irrelevant.
pub fn server_for_connection<'a>(
    servers: &'a [ServerProfile],
    connection: &crate::termius::Connection,
) -> AppResult<&'a ServerProfile> {
    let normalize = |host: &str| {
        host.trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| host.trim().trim_end_matches('.').to_ascii_lowercase())
    };
    let host = normalize(&connection.host);
    let mut matches = servers.iter().filter(|server| {
        normalize(&server.host) == host
            && server.port == connection.port
            && server.username == connection.username
    });
    let matched = matches.next().ok_or_else(|| {
        AppError::new(
            "connection_unmatched",
            "当前真实连接尚未配置。请添加相同主机地址、端口和用户名的服务器。",
        )
    })?;
    if matches.next().is_some() {
        return Err(AppError::new(
            "connection_ambiguous",
            "同一连接配置了多台服务器，请保留唯一的主机、端口和用户名组合。",
        ));
    }
    Ok(matched)
}

fn require_key(profile: &ServerProfile) -> AppResult<()> {
    if !Path::new(&profile.key_file).is_file() {
        return Err(AppError::new(
            "key_missing",
            "专用密钥不存在，请先选择已有密钥或显式生成密钥。",
        ));
    }
    Ok(())
}
fn public_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".pub");
    PathBuf::from(value)
}
fn target(profile: &ServerProfile) -> String {
    format!("{}@{}", profile.username, profile.host)
}
fn scan_target(profile: &ServerProfile) -> String {
    format!("termius-image-paste-probe@{}", profile.host)
}
fn scp_target(profile: &ServerProfile, path: &str) -> String {
    let host = if profile.host.contains(':') {
        format!("[{}]", profile.host)
    } else {
        profile.host.clone()
    };
    format!("{}@{}:{}", profile.username, host, path)
}
fn scp_upload_arguments(
    profile: &ServerProfile,
    local: &Path,
    remote: &str,
) -> AppResult<[String; 5]> {
    Ok([
        "-s".into(),
        "-P".into(),
        profile.port.to_string(),
        local_cli_path(local)?,
        scp_target(profile, remote),
    ])
}
fn config_path(path: &Path) -> AppResult<String> {
    let value = path
        .to_str()
        .ok_or_else(|| AppError::new("invalid_local_path", "本机路径无法表示为 UTF-8。"))?;
    if has_control(value) {
        return Err(AppError::new(
            "invalid_local_path",
            "本机路径包含控制字符。",
        ));
    }
    let value = if cfg!(windows) {
        value.replace('\\', "/")
    } else {
        value.to_string()
    };
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}
fn local_cli_path(path: &Path) -> AppResult<String> {
    let value = path
        .to_str()
        .ok_or_else(|| AppError::new("invalid_local_path", "本机路径无法表示为 UTF-8。"))?;
    #[cfg(windows)]
    {
        if let Some(unc) = value.strip_prefix("\\\\?\\UNC\\") {
            return Ok(format!("\\\\{unc}"));
        }
        if let Some(local) = value.strip_prefix("\\\\?\\") {
            return Ok(local.to_string());
        }
    }
    Ok(value.to_string())
}
fn option_arg(args: &mut Vec<String>, value: &str) {
    args.push("-o".into());
    args.push(value.into());
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

fn parse_scanned_key(content: &str) -> AppResult<String> {
    let mut keys = HashSet::new();
    for line in content.lines().filter(|line| !line.starts_with('#')) {
        let pieces: Vec<_> = line.split_whitespace().collect();
        if pieces.len() >= 3 && pieces[1] == "ssh-ed25519" {
            let key = format!("{} {}", pieces[1], pieces[2]);
            decode_host_key(&key)?;
            keys.insert(key);
        }
    }
    if keys.len() != 1 {
        return Err(AppError::new(
            "scan_failed",
            "未取得唯一有效的 ED25519 主机公钥，请检查地址、端口和网络。",
        ));
    }
    Ok(keys.into_iter().next().unwrap())
}
fn first_public_key(content: &str) -> AppResult<String> {
    let parts: Vec<_> = content.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(AppError::new(
            "invalid_client_key",
            "专用公钥格式无效，已停止授权。",
        ));
    }
    Ok(format!("{} {}", parts[0], parts[1]))
}
fn validate_png(path: &Path) -> AppResult<()> {
    let mut file = File::open(path)
        .map_err(|e| AppError::because("invalid_image", "待上传图片不存在或无法读取。", e))?;
    if !file
        .metadata()
        .map_err(|e| AppError::because("invalid_image", "无法读取图片信息。", e))?
        .is_file()
    {
        return Err(AppError::new("invalid_image", "请选择本机 PNG 图片文件。"));
    }
    let mut signature = [0; 8];
    file.read_exact(&mut signature)
        .map_err(|e| AppError::because("invalid_image", "图片不是有效的 PNG 文件。", e))?;
    if signature != *b"\x89PNG\r\n\x1a\n" {
        return Err(AppError::new(
            "invalid_image",
            "仅接受 PNG 图片，请先转换后上传。",
        ));
    }
    Ok(())
}

struct PasswordServer {
    task: Option<JoinHandle<()>>,
}
impl PasswordServer {
    fn start(
        listener: TcpListener,
        secret_token: String,
        password: Zeroizing<String>,
        token: CancellationToken,
    ) -> Self {
        let task = tokio::spawn(async move {
            let session = async move {
                let mut served = false;
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let (read, mut write) = stream.into_split();
                    let mut reader = BufReader::new(read.take(256));
                    let mut request = Vec::new();
                    let received = tokio::time::timeout(
                        Duration::from_secs(2),
                        reader.read_until(b'\n', &mut request),
                    )
                    .await;
                    if !matches!(received, Ok(Ok(_)))
                        || request != format!("{secret_token}\n").as_bytes()
                        || served
                    {
                        continue;
                    }
                    served = true;
                    let mut response = Zeroizing::new(Vec::with_capacity(password.len() + 1));
                    response.extend_from_slice(password.as_bytes());
                    response.push(b'\n');
                    let _ = write.write_all(&response).await;
                    let _ = write.shutdown().await;
                }
            };
            tokio::select! { _ = token.cancelled() => {}, _ = tokio::time::timeout(Duration::from_secs(90), session) => {} }
        });
        Self { task: Some(task) }
    }
    async fn stop(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}
impl Drop for PasswordServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[derive(Debug)]
struct ProcessOutput {
    stdout: String,
}
fn cancelled() -> AppError {
    AppError::new("cancelled", "操作已取消。")
}

fn find_program(name: &str) -> AppResult<PathBuf> {
    let executable = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.into()
    };
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        let path = PathBuf::from(root)
            .join("System32")
            .join("OpenSSH")
            .join(&executable);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            if !directory.is_absolute() {
                continue;
            }
            let path = directory.join(&executable);
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    Err(AppError::new(
        "openssh_missing",
        &format!("找不到 {executable}，请安装系统 OpenSSH 客户端。"),
    ))
}

async fn read_pipe<R: AsyncRead + Unpin>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let length = reader.read(&mut buffer).await?;
        if length == 0 {
            break;
        }
        let remaining = (64 * 1024usize).saturating_sub(output.len());
        output.extend_from_slice(&buffer[..length.min(remaining)]);
    }
    Ok(output)
}

async fn run_program(
    name: &str,
    args: Vec<String>,
    environment: &[(String, String)],
    timeout: Duration,
    token: CancellationToken,
    allowed_codes: &[i32],
) -> AppResult<ProcessOutput> {
    if token.is_cancelled() {
        return Err(cancelled());
    }
    let mut command = Command::new(find_program(name)?);
    command
        .args(args)
        .envs(environment.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
        #[cfg(target_os = "linux")]
        unsafe {
            let parent = linux_process::getpid();
            command.pre_exec(move || {
                if linux_process::prctl(1, 9, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if linux_process::getppid() != parent {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "parent exited",
                    ));
                }
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| AppError::because("process_start", "无法启动 OpenSSH 子进程。", e))?;
    let mut guard = match ProcessGuard::attach(child.id().unwrap_or(0)) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill().await;
            return Err(error);
        }
    };
    let stdout = tokio::spawn(read_pipe(child.stdout.take().unwrap()));
    let stderr = tokio::spawn(read_pipe(child.stderr.take().unwrap()));
    let result = tokio::select! {
        _ = token.cancelled() => Err(cancelled()),
        _ = tokio::time::sleep(timeout) => Err(AppError::new("timeout", "网络或密钥操作超时，请检查连接后重试。")),
        status = child.wait() => status.map_err(|e| AppError::because("process_wait", "无法读取 OpenSSH 退出状态。", e)),
    };
    guard.terminate();
    if result.is_err() {
        let _ = child.kill().await;
    }
    let stdout = stdout
        .await
        .map_err(|e| AppError::because("process_output", "无法读取 OpenSSH 输出。", e))?
        .map_err(|e| AppError::because("process_output", "无法读取 OpenSSH 输出。", e))?;
    let stderr = stderr
        .await
        .map_err(|e| AppError::because("process_output", "无法读取 OpenSSH 错误信息。", e))?
        .map_err(|e| AppError::because("process_output", "无法读取 OpenSSH 错误信息。", e))?;
    let status = result?;
    if !status
        .code()
        .is_some_and(|code| allowed_codes.contains(&code))
    {
        let detail = String::from_utf8_lossy(&stderr)
            .trim()
            .chars()
            .take(2400)
            .collect::<String>();
        return Err(AppError::because(
            "process_failed",
            &format!("{name} 操作失败，请检查服务器、密钥或连接。"),
            detail,
        ));
    }
    Ok(ProcessOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
    })
}

#[cfg(target_os = "linux")]
mod linux_process {
    unsafe extern "C" {
        pub fn prctl(option: i32, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> i32;
        pub fn getpid() -> i32;
        pub fn getppid() -> i32;
    }
}

#[cfg(unix)]
struct ProcessGuard {
    group: i32,
}
#[cfg(unix)]
impl ProcessGuard {
    fn attach(id: u32) -> AppResult<Self> {
        if id == 0 || id > i32::MAX as u32 {
            return Err(AppError::new(
                "process_isolation",
                "无法隔离 OpenSSH 子进程。",
            ));
        }
        Ok(Self { group: id as i32 })
    }
    fn terminate(&mut self) {
        if self.group > 0 {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            unsafe {
                kill(-self.group, 9);
            }
            self.group = 0;
        }
    }
}
#[cfg(unix)]
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(windows)]
struct ProcessGuard {
    job: usize,
}
#[cfg(windows)]
impl ProcessGuard {
    fn attach(id: u32) -> AppResult<Self> {
        windows_job::attach(id).map(|job| Self { job })
    }
    fn terminate(&mut self) {
        if self.job != 0 {
            windows_job::terminate(self.job);
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if self.job != 0 {
            windows_job::close(self.job);
            self.job = 0;
        }
    }
}

#[cfg(windows)]
mod windows_job {
    use super::*;
    use std::ffi::c_void;
    #[repr(C)]
    struct BasicLimits {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min_set: usize,
        max_set: usize,
        active_limit: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    struct IoCounters {
        values: [u64; 6],
    }
    #[repr(C)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: IoCounters,
        process_memory: usize,
        job_memory: usize,
        peak_process: usize,
        peak_job: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            information: *const c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, code: u32) -> i32;
        fn OpenProcess(access: u32, inherit: i32, id: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    pub fn attach(id: u32) -> AppResult<usize> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(AppError::because(
                    "process_isolation",
                    "无法创建 SSH 进程隔离。",
                    std::io::Error::last_os_error(),
                ));
            }
            let mut limits: ExtendedLimits = std::mem::zeroed();
            limits.basic.flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            let process = OpenProcess(0x0100 | 0x0001, 0, id);
            let success = SetInformationJobObject(
                job,
                9,
                &limits as *const _ as *const c_void,
                std::mem::size_of::<ExtendedLimits>() as u32,
            ) != 0
                && !process.is_null()
                && AssignProcessToJobObject(job, process) != 0;
            let error = std::io::Error::last_os_error();
            if !process.is_null() {
                CloseHandle(process);
            }
            if !success {
                CloseHandle(job);
                return Err(AppError::because(
                    "process_isolation",
                    "无法隔离 SSH 子进程，已停止操作。",
                    error,
                ));
            }
            Ok(job as usize)
        }
    }
    pub fn terminate(job: usize) {
        unsafe {
            TerminateJobObject(job as *mut c_void, 1);
        }
    }
    pub fn close(job: usize) {
        unsafe {
            CloseHandle(job as *mut c_void);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(byte: u8) -> String {
        let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        blob.extend([byte; 32]);
        format!("ssh-ed25519 {}", STANDARD.encode(blob))
    }
    fn profile(directory: &Path) -> ServerProfile {
        ServerProfile {
            host: "example.test".into(),
            username: "testuser".into(),
            key_file: directory.join("id_ed25519").to_string_lossy().into_owned(),
            remote_directory: "/home/testuser/图片 a'b".into(),
            host_key: Some(key(1)),
            ..ServerProfile::default()
        }
    }
    fn settings(directory: &Path) -> Settings {
        let profile = profile(directory);
        Settings {
            active_server_id: Some(profile.id.clone()),
            servers: vec![profile],
            ..Settings::default()
        }
    }
    #[test]
    fn real_connection_selection_ignores_labels_and_requires_unique_full_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        let mut p = profile(directory.path());
        p.name = "arbitrary label".into();
        p.host = "192.0.2.10".into();
        p.username = "user".into();
        p.port = 2222;
        let connection = crate::termius::Connection {
            terminal_id: "terminal".into(),
            connection_id: "connection".into(),
            protocol: "ssh".into(),
            host: p.host.clone(),
            port: p.port,
            username: p.username.clone(),
            target_id: "page".into(),
        };
        assert!(server_for_connection(&[p.clone()], &connection).is_ok());
        for change in 0..3 {
            let mut wrong = p.clone();
            match change {
                0 => wrong.host = "192.0.2.100".into(),
                1 => wrong.port = 22,
                _ => wrong.username = "other".into(),
            }
            assert!(server_for_connection(&[wrong], &connection).is_err());
        }
        assert_eq!(
            server_for_connection(&[p.clone(), p.clone()], &connection)
                .unwrap_err()
                .code,
            "connection_ambiguous"
        );
        p.host = "EXAMPLE.test.".into();
        let mut dns = connection.clone();
        dns.host = "example.test".into();
        assert!(server_for_connection(&[p.clone()], &dns).is_ok());
        p.host = "2001:db8::1".into();
        dns.host = "[2001:0db8::1]".into();
        assert!(server_for_connection(&[p], &dns).is_ok());
    }

    #[test]
    fn remote_directory_response_preserves_names_and_rejects_login_noise() {
        assert_eq!(
            parse_remote_directories("/srv/home\0/srv/home/图片 a'b\0/srv/home/.cache\0").unwrap(),
            vec!["/srv/home", "/srv/home/图片 a'b", "/srv/home/.cache"]
        );
        for invalid in [
            "",
            "\0",
            "/home",
            "welcome\n/home\0",
            "/home\0trailing output",
            "/home\0relative\0",
            "/home\0/bad\nname\0",
        ] {
            assert_eq!(
                parse_remote_directories(invalid).unwrap_err().code,
                "directory_output"
            );
        }
    }

    #[test]
    fn validates_endpoints_and_paths() {
        let directory = tempfile::tempdir().unwrap();
        let mut p = profile(directory.path());
        for host in ["example.test", "127.0.0.1", "2001:db8::1"] {
            p.host = host.into();
            p.validate(true).unwrap();
        }
        for host in [
            "-oProxyCommand=x",
            "a b",
            "host;echo",
            "x@y",
            "[::1]",
            "bad..test",
        ] {
            p.host = host.into();
            assert!(p.validate(true).is_err(), "{host}");
        }
        p.host = "example.test".into();
        p.host_key = None;
        assert_eq!(p.validate(true).unwrap_err().code, "untrusted_host");
        p.remote_directory = "/tmp/path\nnext".into();
        assert!(p.validate(false).is_err());
    }
    #[test]
    fn pin_parser_and_quoting() {
        let pin = key(1);
        assert!(fingerprint(&pin).unwrap().starts_with("SHA256:"));
        assert_eq!(
            parse_scanned_key(&format!("example.test {pin}\n")).unwrap(),
            pin
        );
        assert!(parse_scanned_key(&format!("x {}\nx {}\n", key(1), key(2))).is_err());
        assert!(decode_host_key("ssh-ed25519 Zm9v").is_err());
        assert_eq!(quote_posix("/a b/图'x"), "'/a b/图'\\''x'");
        assert_eq!(legacy_shortcut(6, 123).unwrap(), "Ctrl+Shift+F12");
        assert!(validate_shortcut("Ctrl+Ctrl+A").is_err());
        assert!(validate_shortcut("Alt+Shift+F24").is_ok());
        assert!(validate_shortcut("Ctrl+Super+V").is_ok());
        assert!(validate_shortcut("Super+Shift+F12").is_ok());
        assert!(validate_shortcut("Super+Super+V").is_err());
        assert!(legacy_shortcut(8, 65).is_err());
    }

    #[test]
    fn scan_target_never_uses_unvalidated_username() {
        let directory = tempfile::tempdir().unwrap();
        let mut candidate = profile(directory.path());
        candidate.username = "-oProxyCommand=unexpected-command".into();
        assert_eq!(
            scan_target(&candidate),
            "termius-image-paste-probe@example.test"
        );
        candidate.username.clear();
        assert_eq!(
            scan_target(&candidate),
            "termius-image-paste-probe@example.test"
        );
    }
    #[test]
    fn atomic_settings_and_corruption_preservation() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore {
            config_dir: directory.path().join("config"),
            legacy_path: None,
        };
        assert!(store.load().unwrap().settings.servers.is_empty());
        assert!(!store.settings_path().exists());
        let initial = settings(directory.path());
        store.save(initial.clone()).unwrap();
        assert_eq!(
            store.load().unwrap().settings.servers[0].host_key,
            initial.servers[0].host_key
        );
        let mut changed = initial;
        changed.servers[0].host = "other.test".into();
        assert!(store.save(changed).unwrap().servers[0].host_key.is_none());
        fs::write(store.settings_path(), "{invalid").unwrap();
        assert!(store.load().is_err());
        assert!(store.save(Settings::default()).is_err());
        assert_eq!(
            fs::read_to_string(store.settings_path()).unwrap(),
            "{invalid"
        );
    }
    #[test]
    fn migration_keeps_server_without_personal_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let p = profile(directory.path());
        let value = serde_json::json!({"Servers":[{"Id":p.id,"Name":p.name,"Host":p.host,"Port":p.port,"User":p.username,
            "KeyFile":p.key_file,"RemoteDirectory":p.remote_directory,"HostKey":p.host_key}],"ActiveServerId":p.id,"HotkeyModifiers":6,"HotkeyKey":123,"Enabled":false,"LegacyHotkeyEnabled":true});
        let result = migrate_legacy(serde_json::from_value(value).unwrap()).unwrap();
        assert_eq!(result.servers[0].username, "testuser");
        assert_eq!(result.shortcut, "Ctrl+Shift+F12");
        assert!(!result.enabled);
        assert!(result.legacy_shortcut_enabled);
    }
    #[tokio::test]
    async fn orphan_key_and_cancel_are_safe() {
        let directory = tempfile::tempdir().unwrap();
        let p = profile(directory.path());
        fs::write(public_path(Path::new(&p.key_file)), "preserve").unwrap();
        let client = SshClient::new(directory.path().to_path_buf());
        assert_eq!(
            client
                .generate_key(&p, CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            "key_exists"
        );
        assert_eq!(
            fs::read_to_string(public_path(Path::new(&p.key_file))).unwrap(),
            "preserve"
        );
        let token = CancellationToken::new();
        token.cancel();
        assert_eq!(
            run_program("ssh", vec![], &[], Duration::from_secs(1), token, &[0])
                .await
                .unwrap_err()
                .code,
            "cancelled"
        );
    }
    #[test]
    fn png_signature_and_ipv6_target() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("图片.png");
        fs::write(&file, b"not png").unwrap();
        assert!(validate_png(&file).is_err());
        fs::write(&file, b"\x89PNG\r\n\x1a\n").unwrap();
        validate_png(&file).unwrap();
        let mut p = profile(directory.path());
        p.host = "2001:db8::1".into();
        assert_eq!(
            scp_target(&p, "/tmp/a b.png"),
            "testuser@[2001:db8::1]:/tmp/a b.png"
        );
        p.port = 2222;
        let args = scp_upload_arguments(&p, &file, "/tmp/图片 a'b.png").unwrap();
        assert_eq!(&args[..3], &["-s", "-P", "2222"]);
        assert_eq!(args[3], local_cli_path(&file).unwrap());
        assert_eq!(args[4], "testuser@[2001:db8::1]:/tmp/图片 a'b.png");
    }

    #[test]
    fn no_active_target_and_changed_endpoint_are_explicit() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore {
            config_dir: directory.path().join("config"),
            legacy_path: None,
        };
        let mut config = settings(directory.path());
        config.active_server_id = None;
        store.save(config.clone()).unwrap();
        assert!(store.load().unwrap().settings.active_server().is_none());
        config.servers[0].port = 2222;
        let saved = store.save(config).unwrap();
        assert!(saved.servers[0].host_key.is_none());
        assert!(saved.active_server_id.is_none());
    }

    #[tokio::test]
    async fn real_process_capture_cancel_and_timeout() {
        run_program(
            "ssh",
            strings(&["-V"]),
            &[],
            Duration::from_secs(5),
            CancellationToken::new(),
            &[0],
        )
        .await
        .unwrap();
        let invalid = run_program(
            "ssh",
            strings(&["-Q", "termius-test-invalid"]),
            &[],
            Duration::from_secs(5),
            CancellationToken::new(),
            &[0],
        )
        .await;
        assert_eq!(invalid.unwrap_err().code, "process_failed");
        let ping = if cfg!(windows) {
            strings(&["-n", "30", "127.0.0.1"])
        } else {
            strings(&["-c", "30", "127.0.0.1"])
        };
        let token = CancellationToken::new();
        let trigger = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            trigger.cancel();
        });
        assert_eq!(
            run_program(
                "ping",
                ping.clone(),
                &[],
                Duration::from_secs(5),
                token,
                &[0]
            )
            .await
            .unwrap_err()
            .code,
            "cancelled"
        );
        assert_eq!(
            run_program(
                "ping",
                ping,
                &[],
                Duration::from_millis(200),
                CancellationToken::new(),
                &[0]
            )
            .await
            .unwrap_err()
            .code,
            "timeout"
        );
    }

    #[tokio::test]
    async fn generated_key_and_mismatch_never_reach_server() {
        let directory = tempfile::tempdir().unwrap();
        let client = SshClient::new(directory.path().to_path_buf());
        let profile = profile(directory.path());
        client
            .generate_key(&profile, CancellationToken::new())
            .await
            .unwrap();
        let actual = client
            .authorization_key(&profile, CancellationToken::new())
            .await
            .unwrap();
        assert!(actual.starts_with("ssh-ed25519 "));
        let public = public_path(Path::new(&profile.key_file));
        fs::write(&public, key(99)).unwrap();
        assert_eq!(
            client
                .authorization_key(&profile, CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            "keypair_mismatch"
        );
        assert_eq!(fs::read_to_string(&public).unwrap(), key(99));
        fs::remove_file(public).unwrap();
        assert_eq!(
            client
                .authorization_key(&profile, CancellationToken::new())
                .await
                .unwrap(),
            actual
        );
        let mut no_path = profile.clone();
        no_path.key_file.clear();
        let generated = client
            .generate_key(&no_path, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            Path::new(&generated).parent().unwrap(),
            directory.path().join("keys")
        );
    }

    #[tokio::test]
    async fn password_socket_requires_token_and_serves_once() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = PasswordServer::start(
            listener,
            "fixture-token".into(),
            Zeroizing::new("fixture-password".into()),
            CancellationToken::new(),
        );
        async fn request(address: std::net::SocketAddr, token: &[u8]) -> Vec<u8> {
            let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
            connection.write_all(token).await.unwrap();
            let mut reply = Vec::new();
            tokio::time::timeout(Duration::from_secs(3), connection.read_to_end(&mut reply))
                .await
                .unwrap()
                .unwrap();
            reply
        }
        assert!(request(address, b"wrong-token\n").await.is_empty());
        assert_eq!(
            request(address, b"fixture-token\n").await,
            b"fixture-password\n"
        );
        assert!(request(address, b"fixture-token\n").await.is_empty());
        let lifetime = server.task.as_ref().unwrap().abort_handle();
        server.stop().await;
        // Joining the task proves that its owned listener and zeroizing secret
        // have been dropped; an OS port can meanwhile be reused by another app.
        assert!(lifetime.is_finished());
    }
}
