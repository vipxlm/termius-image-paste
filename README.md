# Termius 贴图助手

[![CI](https://github.com/vipxlm/termius-image-paste/actions/workflows/ci.yml/badge.svg)](https://github.com/vipxlm/termius-image-paste/actions/workflows/ci.yml)

Windows 与 Linux 的本地截图上传工具。把图片上传到选定的 SSH 服务器，再把远程路径带回 Termius 中的对话。使用 Tauri 2、Rust 和系统 OpenSSH，不需要账号或云服务。

## 下载

前往 [GitHub Releases](https://github.com/vipxlm/termius-image-paste/releases/latest) 下载自动构建的正式版本：

| 平台 | 文件与运行方式 |
| --- | --- |
| Windows 10 / 11 x64 | `TermiusImagePaste-windows-x86_64.exe`，直接双击；也提供带说明与许可证的 `.zip`。需要 WebView2 和系统 OpenSSH。 |
| Ubuntu 24.04+ x64 | `termius-image-paste_<版本>_amd64.deb`，用系统软件安装器打开。 |
| Linux 便携程序 | `TermiusImagePaste-linux-x86_64.tar.gz`，解压后运行；需要满足相同系统库依赖。原始 ELF 下载后需要添加执行权限。 |

每次发布附 `SHA256SUMS.txt`。Linux 包针对 Ubuntu 24.04 x86_64 构建，不保证兼容所有发行版；未提供 ARM / macOS 安装包。Windows 程序当前未代码签名。

## 功能

- 中文浅色界面，支持多服务器；Windows 根据 Termius 焦点终端的真实连接自动匹配服务器。
- 按组合键设置快捷键，支持暂停、后台运行与托盘恢复。
- 填写连接信息，选择密码或密钥登录，自动验证并保存；首次连接确认主机身份。
- 窗口内粘贴、拖图或选择图片，显示进度、失败原因与完整远程路径。
- 自动输入保留截图剪贴板，不自动发送消息；输入条件变化时转为手动取用路径。

## 使用

1. 打开软件，在“服务器”中填写主机地址、SSH 端口（默认 22）和用户名。名称可不填，默认使用主机地址。
2. 选择密码或密钥登录，点击“登录并保存”。首次连接核对指纹后自动继续。密码登录自动生成并授权专用密钥，密码不保存；密钥登录使用已有授权。默认图片目录为远端实际用户主目录下的 `.cache/termius-images`，会自动创建。登录成功后可“选择远端目录”浏览文件夹，再登录并保存以验证可写性。
3. Windows：点击 Termius 已连接的终端，会按真实连接的主机地址、端口和用户名选择已保存服务器；名称可任意填写。需要已验证的 Termius 10.2.1 及本机调试接口，详见 [连接识别说明](docs/termius-real-connection.md)。Linux 仍需明确点击“设为当前目标”。
4. 截图后回到 Termius，点击目标消息输入框，按 `Ctrl + Shift + F12`（或自行设置的组合键）。等待路径出现，再发送消息。

也可以在软件窗口内粘贴图片、拖入图片文件或“选择图片”。窗口内上传只显示完整路径，由你自行取用；不会把路径输入到其他窗口。点击“复制路径”会替换剪贴板截图，操作前有明确提示。

## 平台与行为

| 环境 | 行为 |
| --- | --- |
| Windows | 原生前台窗口识别、快捷键与 Unicode 输入。仅在 Termius 前台时占用配置的快捷键。关闭窗口可驻留托盘。 |
| Linux X11 | EWMH 前台窗口识别、全局快捷键和 XTest 输入。整个路径必须可由当前键盘布局表示，否则保留完整路径供手动使用，不部分输入，也不改键盘映射。 |
| Linux Wayland | 窗口内粘贴或选择/拖入图片上传，再手动取用路径。明确停用无法可靠保证的跨应用快捷键和自动输入，不请求远程控制权限。 |

Windows 与 Linux 都需要支持 SFTP 的系统 OpenSSH 客户端（推荐 9.0 或更新版本）。上传显式使用 SFTP；旧客户端不支持时会失败，不回退到远端 shell 传文件。Linux 可见托盘依赖桌面提供 StatusNotifier 宿主；未确认托盘可见时，关闭后保留后台会最小化到任务栏，避免窗口无法找回。系统通知由当前桌面和通知权限决定，完整结果始终留在概览页。

自动贴图保留原截图，不按回车。上传期间离开 Termius、修改已保存设置或取消操作后，不会自动补输路径；已上传的路径仍会展示。Windows 从焦点终端读取连接快照，并按主机地址、端口、用户名精确匹配唯一配置；不读取标签名，不回退到旧目标。上传前和填入路径前重新核对窗口、终端及连接标识；接口不可用、广播开启、字段不完整或匹配不唯一时停止。分屏按获得输入焦点的终端识别；在终端内再次执行 SSH 的目标不属于此快照，暂不支持。Linux 保持手动选目标。

## 连接、密钥与本地数据

- 每台服务器独立保存 SSH 主机公钥，正式连接严格校验指纹。修改主机或端口后需要重新核对。发现主机身份变化时，不自动替换信任记录。
- 指纹读取使用无认证的系统 SSH 探测和独立临时文件，随后由用户核对。它不登录、不保存信任、不修改远端授权。
- “登录并保存”自动验证连接和目录。密码登录会追加专用公钥，后续贴图使用该密钥；已有授权保留。默认目录自动创建，自选目录须可写，密码首次配置时可创建所填目录。浏览目录只读取文件夹列表。
- 密码登录自动生成或复用 ED25519 专用密钥；已有私钥、公钥或不匹配的密钥对不会被覆盖。首次授权支持普通 SSH 密码，不自动处理 MFA、OTP 或私钥口令；这些情况请先在 Termius 中手动完成授权。
- 密码只用于本次操作，不写入配置、日志、命令行或环境变量。OpenSSH 密码辅助进程通过短时的一次性本机通道获取密码。
- 首次启动可读取旧 Windows 版 `%LOCALAPPDATA%\TermiusImagePaste\settings.json`，复用已有服务器、指纹、专用密钥路径和快捷键。旧配置与密钥保留；新版保存到独立目录。原文件损坏时停止保存，不用默认配置覆盖。
- 新配置目录由 Tauri 的 `app_config_dir` 决定，界面“快捷键与偏好”显示实际位置。Windows 通常为 `%APPDATA%\com.sean.termius-image-paste`，Linux 通常为 `$XDG_CONFIG_HOME/com.sean.termius-image-paste`（未设置时为 `~/.config/...`）。只保存本机设置，不同步私钥。
- 本地临时 PNG 在操作完成后清理，远端图片保留。最近一次操作结果只保留在本次运行内存中。

## 构建与验证

使用 Node.js 24 与 `rust-toolchain.toml` 固定的 Rust 1.99.1，以及 [Tauri 官方构建依赖](https://v2.tauri.app/start/prerequisites/)。前端是本地 HTML/CSS/JavaScript，未引入前端 UI 框架。Windows 需要 MSVC Build Tools 与 WebView2；Linux 需要 WebKitGTK 4.1、GTK 3、AppIndicator 和 XTest 等开发包。

```powershell
npm ci
npm test
npm run dev
npm run build
```

便携二进制可用 `npm run build -- --no-bundle` 构建。输出在 `src-tauri/target/release/`；各平台分别构建，Linux 产物的系统库最低要求取决于构建发行版。构建和单元测试成功不能代替 Termius、具体桌面、键盘布局及系统权限的实机验收。

测试使用临时配置、临时生成的测试密钥和回环地址，不读取用户私钥内容，不修改实际服务器授权。检查包括配置迁移与损坏保留、字段校验、主机信任、取消与超时、密码辅助通道、密钥错配、窗口与路径输入保护。

[CI](https://github.com/vipxlm/termius-image-paste/actions/workflows/ci.yml) 检查 Windows 与 Ubuntu 24.04；推送与项目版本一致的 `vX.Y.Z` 标签后，[Release 工作流](https://github.com/vipxlm/termius-image-paste/actions/workflows/release.yml) 自动测试、构建并发布两平台产物。任一平台失败就不公开新的 Release。具体步骤见 [发布说明](RELEASING.md)，参与开发见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## Linux 安装包

首版提供 Ubuntu 24.04 及更新版本的 x86_64 `.deb` 安装包，可用系统的软件安装器打开。便携 ELF 使用同一套系统库，也需要满足依赖；不宣称兼容所有 Linux 发行版或 ARM。

在满足依赖的 Ubuntu 24.04 x86_64 环境中运行 `bash tools/package-linux.sh`，会生成 `dist/TermiusImagePaste-linux-x86_64`、`.deb` 和 SHA-256 校验文件。脚本直接用 Cargo 构建带 `tauri/custom-protocol` 的本地界面，不需要 Linux Node；开发界面与 Tauri 安装器仍可使用上面的 npm 命令。

## 许可证与项目关系

本项目代码使用 [MIT 许可证](LICENSE)。第三方依赖保留各自的许可证，项目许可证不替代其许可要求。发布归档附带项目许可与按依赖源码收集的许可文本清单，参见 [第三方声明](THIRD_PARTY_NOTICES.md)。

这是独立的非官方辅助工具，与 Termius 没有隶属或背书关系；Termius 名称和商标归其权利人所有。

服务器表单逻辑检查：`node tools/test-server-editor.mjs`（使用隔离的 DOM 和命令桩，不连接真实服务器）。

## Windows 自动切换的验证

1. 配置并登录两台服务器；Termius 标签和服务器显示名称可任意填写。
2. 切到第一台终端，确认助手显示真实地址及对应服务器；切到第二台，确认自动更新。
3. 分屏分别点击终端，确认跟随输入焦点；开启广播时应拒绝上传。
4. 未配置或存在重复地址、端口、用户名组合时，应拒绝上传；关闭调试接口也必须拒绝识别。
5. 上传前切换连接应停止上传；上传过程中切换连接应阻止路径自动输入。

2026-10-18 已在本机 Termius 10.2.1 验证焦点连接读取和两台服务器切换。分屏实际操作、真实图片上传及路径输入仍需运行时验收；模拟测试不替代这些检查。原生只读检查：设置 `TERMIUS_PROCESS_ID` 后运行 `cargo test --manifest-path src-tauri/Cargo.toml live_connection_readonly -- --ignored`，并保持终端焦点。探针回归检查：`node --test tools/test-termius-connection-probe.mjs`。
