# Termius 真实连接识别：调查与待实测探针

确认日期：2026-10-18。当前研究分支只准备读取探针，尚未替换应用中的标签匹配逻辑，不能作为真实连接识别已经交付的证明。

## 已确认的静态事实

目标：本机安装的 Termius 10.2.1。`app.asar` SHA-256：`dc2f4bf69399bf0093f50143702e9760073d1df53b5bf795a6d6849741d0f08c`。

- `ui-process/assets/reconnectSaga-9aed66e2.js` 中，`createTerminalObject` 分别记录 `id`、`windowId`、`connectionId`、`isActive` 和 `connectionEstablished`。`getActiveTerminal` 按窗口及 `isActive` 查终端；`ACTIVATE_TERMINAL` 更新活动终端。
- 同文件的 `Connection` 对象保存 `host.address`，`port` 与 `username` getter 从合并后的实际连接配置获取值。名称并非识别所需字段。
- `ui-process/assets/ui-process-db39bd97.js` 的终端焦点回调按终端 ID 激活终端。React 组件显式接收 `mediator`；其 `currentConnection` 返回当前连接对象，`isShellReady` 表明 shell 对象是否存在。这些事实提供焦点分屏与连接关联的候选路径，尚未在当前运行实例中验证。
- 主进程为界面页附加 `type`、`id` 查询参数，探针按 URL pathname 筛选界面页，不把标签名或 URL 查询参数当主机地址。
- 当前 Termius 进程没有启动 Inspector/CDP 参数，本次检查未发现由这些进程监听的 TCP 端口。
- REA `inspect_electron_fuses` 证据 `ev_bd975d320b67337c7d35e661d4ac3a41c4869afa54a3c4d59f99ba4e8c6f6a4a`：`EnableNodeCliInspectArguments` 禁用，ASAR 完整性校验和仅从 ASAR 加载启用。不能假定 `--inspect` 可用，不应为此修改二进制或安装包。此结论不证明 Chromium renderer 的 `--remote-debugging-port` 被禁用。

## 已准备的探针

`tools/probe-termius-connection.mjs` 使用本机已有 Node.js 24 的 fetch/WebSocket，无新增依赖。

它只接受显式的 `http://127.0.0.1:端口/`；从同一端口选择 Termius UI 页面，读取 document 焦点及焦点终端祖先的 React 关联；只投影主机、端口、用户名、终端 ID、连接 ID、协议和广播检查状态。不会输出连接对象、身份对象、密码、私钥、终端正文或整个应用状态；不会向终端输入命令。

模糊焦点、多连接候选、广播输入、非 SSH/mosh、缺失字段、版本不适配及 getter 副作用检查失败时均停止，不回退到标题或默认主机。`broadcastChecked=false` 只供诊断，不能直接接入自动上传。

独立检查：

```powershell
node --test tools/test-termius-connection-probe.mjs
```

这属于模拟 DOM、React、连接对象和 CDP 的测试。它不证明实际 Termius 运行时读取成功。

## 待批准的运行时验证

需要先由用户批准关闭并重启正在使用的 Termius；现有 SSH 会话可能中断。准备尝试的启动参数为：

```text
--remote-debugging-port=9222 --remote-debugging-address=127.0.0.1
```

启动前确认端口未被占用；启动后核实监听进程归属及仅绑定回环地址，检查失败即停止验证，不能假定参数必定生效。不增加永久启动配置，不修改 ASAR/fuse，不开放局域网端口。调试接口自身权限高于这个探针，只在获批的短时验证中启用；正常退出并不带这些参数重启可关闭接口。

确认接口后，探针调用示例（本次未执行）：

```powershell
node tools/probe-termius-connection.mjs http://127.0.0.1:9222/ 20
```

在观察期间切回 Termius，分别点击两个同名独立标签和分屏中的终端，比较真实主机/端口/用户名与连接 ID。然后核实重新连接、断开连接、广播模式和焦点离开终端的行为。真实服务器明细与探针输出只在本地检查，不加入 Git 或长期记忆。

## 通过后才接入产品

- 用焦点终端连接快照匹配已保存服务器的主机、端口及用户名；不再依赖标签名，匹配不到或不唯一时停止。
- 上传前后比较窗口、终端 ID、连接 ID 和连接参数，避免切换分屏或重连后填入旧路径。
- 接口不可用、广播状态未知或不支持的 Termius 版本时，不猜测服务器，也不启用标签匹配作为隐式回退。
- 产品中的连接方式、用户端启用流程和调试接口生命周期尚未决定；探针只是为选择可行实现提供实测证据，不是新增常驻服务。

## 在终端内再次 SSH 的边界

Termius 管理的外层 SSH 到 A，不等于 A 内部再执行 SSH 到 B 后的最内层 shell。读取 `currentConnection` 只能得到 Termius 管理的连接，不能证明最内层已经是 B。

若要求自动跟随最内层，需要远端配合：由每台获授权服务器的 shell 集成或代理报告可验证的机器身份和 shell 生命周期，并与当前终端通道关联。仅解析 `ssh ...` 命令、提示符、窗口标题或一次 `hostname` 输出都不足以可靠识别别名、跳板、容器及多层连接。远端安装、shell 配置修改及身份协议须另行给出具体改动并获批；本次没有执行。

参考：[Electron 调试端口参数](https://www.electronjs.org/docs/latest/api/command-line-switches#--remote-debugging-portport)、[Electron fuse 说明](https://www.electronjs.org/docs/latest/tutorial/fuses#nodecliinspect)。
