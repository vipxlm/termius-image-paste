# Termius 焦点终端真实连接识别

此功能从 `v2.1.0` 引入，旧版 `v2.0.0` 不包含。Windows 自动识别读取 Termius 管理的连接，不依赖标签名；兼容范围于 2026-10-18 核验。

## 使用前准备

1. 确认 Termius 为已适配的 10.2.1 安装包，其他构建会因指纹不同而拒绝识别。
2. 保存必要工作，完全退出 Termius。重启会影响现有 SSH 会话。
3. 使用下方示例在本机回环地址启动调试接口，然后重新连接服务器。若安装位置不同，请调整路径；端口需要空闲。
4. 在助手中保存相同主机地址、端口、用户名的配置，回到 Termius 点击终端输入区域。名称可以任意填写。

```powershell
& "$env:LOCALAPPDATA\Programs\Termius\Termius.exe" --remote-debugging-port=9223 --remote-debugging-address=127.0.0.1
```

识别失败时不会切回旧服务器。终端内再次 SSH 不属于此机制的识别范围，见文末说明。

## 读取路径与约束

适配本机 Termius 10.2.1，安装包 `resources/app.asar` 的 SHA-256 为 `dc2f4bf69399bf0093f50143702e9760073d1df53b5bf795a6d6849741d0f08c`。原生端核验当前前台 Termius 进程、安装包指纹以及回环监听端口的所属 PID，使用 Windows 自带 WinHTTP/WebSocket，无新增依赖或服务。未知安装包拒绝识别。

从 `/json/list` 按文件页面路径选择 UI，使用 `Runtime.evaluate`，始终启用 `throwOnSideEffect: true`。只读取输入焦点终端祖先上的 React mediator、当前连接、广播状态和连接端点必要字段，不读取密码、私钥、终端正文或导出整个状态。

共享读取函数位于 `src-tauri/src/termius-connection-probe.js`。直接调用 DOM closest 或合并配置构造 getter 会被当前 V8 副作用检查拒绝，因此通过父元素定位 xterm，并按已核验的配置继承规则只读取主机/分组端口和用户名。空的私有身份可能需要凭据才能判断继承，直接拒绝；不为方便识别读取凭据。默认端口 22 来自该版本的全局默认配置。

用真实主机地址、端口、用户名唯一匹配已保存配置。DNS 名称只做大小写和结尾点规范化，IP 使用原生解析规范化；不把未验证的 DNS 别名猜成另一地址。显示名称任意填写，同名标签不参与判断。广播开启、广播状态未知、非终端焦点、连接未就绪、字段不完整、窗口候选不唯一或配置不唯一均停止，不回退到标签或旧服务器。

上传前和路径输入前复核前台窗口、进程、终端 ID、连接 ID、地址、端口、用户名以及设置代次；连接变化时停止上传或停止自动输入。连接快照是瞬时读数，不能消除最后一次检查与操作之间的所有竞态。

## 调试接口的生命周期

当前方案依赖 Termius 已启动的本机 Chromium 调试接口。示例使用端口 9223；应用按进程动态发现回环端口，不将端口固定在配置中。启动参数为：

```text
--remote-debugging-port=9223 --remote-debugging-address=127.0.0.1
```

启用前应确认端口空闲；应用只连接 Termius 所属的 127.0.0.1 监听端口。调试接口权限高于此读取器，仅在需要且信任本机环境时启用，不向局域网开放。正常退出并不带调试参数重新启动 Termius 可关闭接口，此后自动识别会停止；应用不会自行重启 Termius 或永久开启接口。

## 验证证据

已通过 Windows 原生 WinHTTP/WebSocket 只读验收（1 项通过），以及共享探针的实际前台采样，观察到两个服务器之间切换时端点、终端 ID 和连接 ID 随之变化。真实端点及标识未加入 Git。代码检查覆盖任意同名标题、焦点丢失、广播、多个候选、继承和默认端口、不完整字段、端口与回环约束以及渲染错误脱敏。

```powershell
node --test tools/test-termius-connection-probe.mjs
node tools/test-server-editor.mjs
cargo test --locked --manifest-path src-tauri/Cargo.toml
```

原生只读验收：设置 `TERMIUS_PROCESS_ID` 为对应主进程 PID，运行 `cargo test --manifest-path src-tauri/Cargo.toml live_connection_readonly -- --ignored`，并保持终端焦点。此检查不上传图片、不输入命令。

实际分屏焦点、同名标签、广播切换、重连、图片上传以及自动路径输入需要使用新程序继续验收，模拟测试不能代替这些行为。

## 在终端内再次 SSH 的边界

Termius 管理的外层 SSH 到 A，不等于 A 内再 SSH 到 B 后的最内层 shell。当前连接快照只能得到 Termius 管理的连接，不能证明最内层为 B。

自动跟随最内层需要远端配合并与终端通道绑定的可验证机器身份和 shell 生命周期。仅解析命令、提示符、标题或一次 hostname 不足以可靠识别多层连接。这需要另行设计并部署远端集成，当前版本未提供。

原生通道使用 [Microsoft WinHTTP WebSocket API](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpwebsocketreceive)。WebSocket 句柄不接受普通 RECEIVE_TIMEOUT 选项，使用有界协议关闭取消等待；原生句柄保留至关闭线程结束。
