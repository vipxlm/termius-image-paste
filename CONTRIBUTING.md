# 参与开发

欢迎通过 Issue 报告问题，或提交 Pull Request。报告问题时请注明系统、桌面环境（Linux 的 X11 / Wayland）、软件版本、操作步骤与预期结果。请把真实服务器地址、用户名、主机指纹、截图中的隐私及本机路径脱敏；不要提交密码、SSH 私钥或完整个人配置。

## 本地开发

安装 Node.js 24、项目固定的 Rust 工具链及 [Tauri 官方系统依赖](https://v2.tauri.app/start/prerequisites/)，然后执行：

```sh
npm ci
npm run dev
```

Rust 版本由 `rust-toolchain.toml` 固定，依赖由两个锁文件固定。更换版本或依赖时请一并说明原因，并验证 Windows 与 Ubuntu 24.04 构建。Linux 系统库与打包步骤可参考 `.github/workflows/ci.yml` 和 `tools/package-linux.sh`。

提交前执行：

```sh
node --check ui/app.js
node tools/check-version.mjs
node tools/test-server-editor.mjs
node --test tools/test-termius-connection-probe.mjs
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --locked --manifest-path src-tauri/Cargo.toml
```

Linux 上可用 `xvfb-run -a cargo test --locked --manifest-path src-tauri/Cargo.toml` 执行现有 X11 回归测试。测试不会连接你的实际服务器或修改其授权；默认忽略的真实连接测试不应在 CI 或外部 PR 中启用。

## 改动边界

- 截图剪贴板保留，自动输入不按回车；跨窗口、取消和设置变化应使待输入结果失效。
- Windows 只按焦点终端真实连接唯一匹配已保存服务器，不使用标签名或旧目标回退；Linux 保持手动选择。编辑中的连接不能冒充已保存配置。
- 正式 SSH 连接严格验证已确认的主机身份。改地址、端口或主机密钥后必须重新核对。
- Wayland 首版使用窗口内上传与手动取路径；能力受限时明确提示，不显示模拟成功。
- 构建与测试成功不代表真实 Termius、具体输入法或桌面权限已验收。涉及这些行为的 PR 请注明实际测试环境。

提交的项目代码按仓库 MIT 许可证提供；第三方代码与素材须保留其原始许可和来源。
