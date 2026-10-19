# 自动构建与发布

## 日常检查

Pull Request 与默认分支更新由 `ci.yml` 检查 Windows 和 Ubuntu 24.04：版本一致性、前端语法、Rust 格式及现有测试。外部 PR 不取得发布写权限，也不使用个人服务器、密钥或密码。

## 发布正式版本

1. 在开发分支同时更新 `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock` 与 `src-tauri/tauri.conf.json` 中的项目版本；运行版本检查和测试。
2. 通过 Pull Request 合并到默认分支，等待 CI 通过。
3. 在该提交上创建并推送 `vX.Y.Z` 标签，例如：

   ```sh
   git tag v2.0.1
   git push origin v2.0.1
   ```

4. `release.yml` 会先验证标签与三个版本声明相同，再分别测试、构建 Windows x64 和 Ubuntu 24.04 x64 产物，收集依赖的许可文本。两边成功后，独立发布任务上传程序资产、两平台的许可文本和 SHA-256 清单，最后公开 GitHub Release。

标签触发并不代表构建成功；应查看 Actions 和 Release 实际资产。任一平台失败就先修复并检查日志，不手动把本机旧二进制冒充本次云端构建。已正式发布的同版本资产不自动覆盖；修复后使用新版本号。

## 范围

发布的是 Windows 便携程序与 Linux ELF / `.deb`，Linux 依赖下限由 Ubuntu 24.04 构建环境决定。没有发布 macOS 或 ARM。当前源码的 Windows x64 便携正式版支持在线更新；Linux 与开发模式继续手动更新。Windows 程序未附带代码签名证书；本流程不需要额外上传个人 PAT 或 SSH 密钥。

构建任务只有读取源码的权限，发布写权限只交给不执行项目构建的发布任务，通过 GitHub 自动提供的 `GITHUB_TOKEN` 使用。第三方 Actions 固定到官方仓库的精确提交，升级时核对官方发行记录。

## Windows 在线更新的资产约定

保留 `TermiusImagePaste-windows-x86_64.exe` 与 `SHA256SUMS.txt` 的固定资产名，以及标准 `vX.Y.Z` 正式标签。更新器从本仓库 `/releases/latest` 获取版本及 GitHub 自动生成的 EXE `digest`，下载后同时核对 SHA-256 清单。资产缺失、重名、非本仓库 URL、摘要缺失或校验失败时不应用更新。

不需要额外的更新服务、密钥或新发布资产。当前信任边界为 GitHub 仓库发布权限、GitHub HTTPS 和资产摘要；没有独立离线发布签名。后续若更改资产名、目标架构或更新记录结构，需保留已发布更新器的兼容行为。

本地针对测试：`cargo test --locked --manifest-path src-tauri/Cargo.toml` 包含真实更新助手子进程的隔离替换、等待退出、重启和篡改拒绝测试。显式网络检查可运行 `cargo test --locked --manifest-path src-tauri/Cargo.toml updater::tests::live_github_download_to_temporary_directory -- --ignored --exact`，仅下载到临时目录，不替换或启动已安装客户端。
