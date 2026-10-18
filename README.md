# Termius 贴图助手

[![CI](https://github.com/vipxlm/termius-image-paste/actions/workflows/ci.yml/badge.svg)](https://github.com/vipxlm/termius-image-paste/actions/workflows/ci.yml)

把本地截图上传到 SSH 服务器，再将图片路径填入 Termius。适合在远程终端里向支持读取图片路径的工具提供截图。

**截图 → 回到 Termius → 按 `Ctrl + Shift + F12` → 等待路径出现 → 自行发送。**

支持 Windows 与 Linux，使用本机 OpenSSH 上传，无需额外账号或云存储。自动填入路径时保留原截图，不按回车。

## 下载与版本

前往 [GitHub Releases](https://github.com/vipxlm/termius-image-paste/releases/latest) 下载正式版。

| 平台 | 下载文件 | 运行要求 |
| --- | --- | --- |
| Windows 10 / 11 x64 | `TermiusImagePaste-windows-x86_64.exe` 或 `.zip` | WebView2、系统 OpenSSH |
| Ubuntu 24.04 x64 | `termius-image-paste_<版本>_amd64.deb` | 用系统软件安装器安装 |
| Linux x64 便携版 | `TermiusImagePaste-linux-x86_64.tar.gz` | 满足与 Ubuntu 构建产物相同的系统库依赖 |

发布资产附带 `SHA256SUMS.txt`。Windows 程序尚未代码签名；未提供 macOS 或 ARM 安装包，Linux 不保证兼容所有发行版。

> **正式版与当前源码不同。** 截至 2026-10-18，最新正式版为 [v2.0.0](https://github.com/vipxlm/termius-image-paste/releases/tag/v2.0.0)。下文介绍的简化登录、远端目录选择及 Windows 真实连接自动识别已经合并到 `main`，尚未发布到 Releases。体验这些功能需要从当前源码构建；旧版下载包不会随 README 更新而升级。

## 当前源码的使用方式

### 1. 添加服务器

在“服务器”中填写主机地址、SSH 端口（默认 `22`）和用户名，选择密码或密钥登录，点击“登录并保存”。名称选填，仅用于显示。

首次连接需要核对主机指纹。密码登录会自动准备专用密钥并完成授权，密码不保存；密钥登录使用你已有的私钥及服务器授权。

图片默认保存在远端用户主目录下的 `.cache/termius-images`。也可以在登录后选择远端目录，再登录并保存以确认可写。详见 [配置、目录和本地数据](docs/usage.md)。

### 2. 确认上传目标

| 环境 | 目标选择 | 贴图方式 |
| --- | --- | --- |
| Windows | 从 Termius 焦点终端读取真实连接，按主机、端口和用户名匹配已保存配置 | Termius 前台快捷键上传并填入路径 |
| Linux X11 | 在助手中点击“设为当前目标” | Termius 前台快捷键上传并填入路径；受键盘布局限制 |
| Linux Wayland | 在助手中手动选择目标 | 窗口内上传，再手动复制路径 |

**Windows 自动识别有前提：** 当前仅适配已核验的 Termius 10.2.1 安装包，并要求 Termius 已开启本机回环调试接口。普通启动、未适配版本或识别失败时会停止自动贴图，不根据标签名猜测服务器。启动方法和限制见 [Windows 连接识别说明](docs/termius-real-connection.md)。

### 3. 上传截图

截图后切回 Termius，点击已连接服务器的终端输入区域，按 `Ctrl + Shift + F12`。确认路径出现后，再自行发送消息。快捷键可在助手中修改，也可以暂停。

也可以在助手窗口内粘贴、拖入或选择图片；窗口内上传只显示路径，由你自行使用。“复制路径”会替换剪贴板中的截图，操作前会提示。

## 自动贴图的边界

- **目标必须唯一。** Windows 按地址、端口和用户名识别，显示名称和标签名不参与匹配；重复配置、字段缺失或广播输入开启时停止操作。
- **切换连接会重新检查。** 上传前及填入路径前核对目标；上传过程中切换连接，已经上传的文件保留，但不会自动补输旧路径。
- **终端内再次 SSH 暂不支持。** Termius 连接 A 后，在 A 中执行 SSH 到 B，当前识别仍是 Termius 管理的 A。
- **上传文件会保留。** 本地临时图片在操作结束后清理，远端图片由你管理。删除助手中的服务器配置不会删除远端图片或撤销已有密钥授权。

## 开发与更多说明

使用 Node.js 24、仓库固定的 Rust 工具链和 [Tauri 构建依赖](https://v2.tauri.app/start/prerequisites/)：

```sh
npm ci
npm run dev
```

构建使用 `npm run build`，测试使用 `npm test`。完整检查与平台依赖见 [参与开发](CONTRIBUTING.md)，打包和版本发布见 [发布说明](RELEASING.md)。本项目使用 Tauri 2、Rust 和本地 HTML/CSS/JavaScript。

- [配置、目录和本地数据](docs/usage.md)
- [Windows 连接识别、兼容范围与验证](docs/termius-real-connection.md)
- [第三方许可声明](THIRD_PARTY_NOTICES.md)

项目代码采用 [MIT 许可证](LICENSE)。这是非官方辅助工具，与 Termius 没有隶属或背书关系；相关名称和商标归其权利人所有。
