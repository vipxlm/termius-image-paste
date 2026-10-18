import { pathToFileURL } from 'node:url';

// This function is serialized into a fixed, read-only renderer expression.
// Never serialize a host, identity, store, mediator, or React props object.
export function readFocusedConnection(doc = document) {
  if (!doc.hasFocus()) return { status: 'not-focused' };
  const focused = doc.activeElement;
  if (!focused?.closest?.('.xterm')) return { status: 'terminal-not-focused' };
  const mediators = new Set();
  const stores = new Set();
  const visited = new Set();
  let budget = 512;
  for (let element = focused; element && budget > 0; element = element.parentElement) {
    const key = Object.getOwnPropertyNames(element).find(name => name.startsWith('__reactFiber$'));
    for (let fiber = key ? element[key] : null; fiber && !visited.has(fiber) && budget > 0; fiber = fiber.return) {
      visited.add(fiber); budget--;
      const props = fiber.memoizedProps;
      if (!props || typeof props !== 'object') continue;
      if (props.mediator?.terminalApi && props.mediator.currentConnection) mediators.add(props.mediator);
      const store = props.store || props.value?.store;
      if (typeof store?.getState === 'function') stores.add(store);
    }
  }
  if (budget <= 0) return { status: 'inspection-limit' };
  if (mediators.size !== 1) return { status: mediators.size ? 'ambiguous-terminal' : 'terminal-adapter-unavailable' };
  const mediator = [...mediators][0];
  const connection = mediator.currentConnection;
  if (!mediator.isShellReady) return { status: 'shell-not-ready' };
  if (!['ssh', 'mosh'].includes(connection.protocol)) return { status: 'unsupported-protocol' };
  // Broadcast input must never be treated as one uniquely addressed terminal.
  let broadcastChecked = false;
  for (const store of stores) {
    const terminal = store.getState()?.terminal;
    const broadcasts = terminal?.splitViewBroadcastTabs;
    if (broadcasts && typeof broadcasts.size === 'number') {
      broadcastChecked = true;
      if (broadcasts.size !== 0) return { status: 'broadcast-active' };
    }
  }
  const singleLine = (value, limit) => typeof value === 'string' && value.length > 0 && value.length <= limit && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
  const host = connection.host?.address;
  const port = connection.port;
  const username = connection.username;
  const terminalId = mediator.terminalApi.id;
  const connectionId = connection.id;
  if (!singleLine(host, 255) || !singleLine(username, 256) || !singleLine(terminalId, 256) || !singleLine(connectionId, 256)
      || !Number.isInteger(port) || port < 1 || port > 65535) return { status: 'incomplete-connection' };
  return {
    status: 'connection-found', source: 'focused-terminal-connection',
    terminalId, connectionId, protocol: connection.protocol, host, port, username,
    broadcastChecked, scope: 'termius-managed-connection'
  };
}

export function validateEndpoint(input) {
  const endpoint = new URL(input);
  if (endpoint.protocol !== 'http:' || endpoint.hostname !== '127.0.0.1' || !endpoint.port
      || endpoint.username || endpoint.password || endpoint.pathname !== '/' || endpoint.search || endpoint.hash) {
    throw new Error('仅接受 http://127.0.0.1:端口/，不接受远端地址、凭据、路径或查询参数。');
  }
  return endpoint;
}

export function validateSocket(input, endpoint) {
  const socket = new URL(input);
  if (socket.protocol !== 'ws:' || socket.hostname !== endpoint.hostname || socket.port !== endpoint.port
      || socket.username || socket.password || socket.search || socket.hash
      || !/^\/devtools\/page\/[A-Za-z0-9-]+$/.test(socket.pathname)) throw new Error('调试目标没有使用指定的本机端口。');
  return socket.href;
}

export function projectObservation(value) {
  const states = new Set(['not-focused', 'terminal-not-focused', 'inspection-limit', 'ambiguous-terminal', 'terminal-adapter-unavailable', 'shell-not-ready', 'unsupported-protocol', 'broadcast-active', 'incomplete-connection', 'connection-found']);
  if (!value || !states.has(value.status)) throw new Error('探针返回了未识别的状态。');
  if (value.status !== 'connection-found') return { status: value.status };
  const line = input => typeof input === 'string' && input.length > 0 && input.length <= 256 && !/[\u0000-\u001f\u007f-\u009f]/.test(input);
  if (![value.host, value.username, value.terminalId, value.connectionId].every(line)
      || !Number.isInteger(value.port) || value.port < 1 || value.port > 65535 || !['ssh', 'mosh'].includes(value.protocol)) throw new Error('连接字段不完整。');
  return { status: 'connection-found', source: 'focused-terminal-connection', terminalId: value.terminalId, connectionId: value.connectionId,
    protocol: value.protocol, host: value.host, port: value.port, username: value.username,
    broadcastChecked: value.broadcastChecked === true, scope: 'termius-managed-connection' };
}

export function probeTarget(socketUrl, WebSocketClass = WebSocket) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocketClass(socketUrl);
    const timer = setTimeout(() => finish(new Error('读取终端连接超时。')), 2500);
    let finished = false;
    function finish(error, value) {
      if (finished) return;
      finished = true; clearTimeout(timer);
      socket.close();
      if (error) reject(error); else resolve(value);
    }
    socket.addEventListener('open', () => socket.send(JSON.stringify({
      id: 1, method: 'Runtime.evaluate', params: {
        expression: `(${readFocusedConnection.toString()})()`,
        returnByValue: true, silent: true, throwOnSideEffect: true
      }
    })));
    socket.addEventListener('message', event => {
      let message;
      try { message = JSON.parse(String(event.data)); } catch { finish(new Error('调试接口返回无效数据。')); return; }
      if (message.id !== 1) return;
      // Do not copy raw exceptions or arbitrary renderer objects into logs.
      if (message.error || message.result?.exceptionDetails) { finish(new Error('只读探针执行失败；可能存在版本差异或不允许的 getter 副作用。')); return; }
      const value = message.result?.result?.value;
      if (!value || typeof value.status !== 'string') { finish(new Error('调试接口未返回探针结果。')); return; }
      try { finish(null, projectObservation(value)); } catch { finish(new Error('调试接口未返回有效的连接字段。')); }
    });
    socket.addEventListener('error', () => finish(new Error('无法连接本机 Termius 调试接口。')));
    socket.addEventListener('close', () => finish(new Error('Termius 调试连接已关闭。')));
  });
}

export async function inspectOnce(endpoint, fetchImpl = fetch, WebSocketClass = WebSocket) {
  const response = await fetchImpl(new URL('/json/list', endpoint), { redirect: 'error', signal: AbortSignal.timeout(2500) });
  if (!response.ok) throw new Error('无法读取本机调试目标。');
  const text = await response.text();
  if (text.length > 1024 * 1024) throw new Error('调试目标列表过大。');
  const inventory = JSON.parse(text);
  if (!Array.isArray(inventory)) throw new Error('调试目标列表格式无效。');
  const targets = inventory.filter(target => {
    if (target.type !== 'page' || typeof target.url !== 'string') return false;
    try {
      const url = new URL(target.url);
      return url.protocol === 'file:' && !url.host && url.pathname.endsWith('/ui-process/src/ui-process/main.html');
    } catch { return false; }
  });
  if (targets.length > 8) throw new Error('Termius 窗口过多，探针停止，避免只检查部分窗口后误选。');
  if (!targets.length) return { status: 'termius-page-unavailable' };
  const observations = [];
  for (const target of targets) observations.push(await probeTarget(validateSocket(target.webSocketDebuggerUrl, endpoint), WebSocketClass));
  const focused = observations.filter(value => value.status !== 'not-focused');
  if (focused.length !== 1) return { status: focused.length ? 'ambiguous-window' : 'not-focused' };
  return focused[0];
}

async function main(args) {
  if (args.length < 1 || args.length > 2) throw new Error('用法：node tools/probe-termius-connection.mjs http://127.0.0.1:端口/ [观察秒数1-60]');
  const endpoint = validateEndpoint(args[0]);
  const seconds = args[1] === undefined ? 1 : Number(args[1]);
  if (!Number.isInteger(seconds) || seconds < 1 || seconds > 60) throw new Error('观察时间须为 1–60 秒。');
  const deadline = Date.now() + seconds * 1000;
  let previous = '';
  do {
    const result = JSON.stringify(await inspectOnce(endpoint));
    if (result !== previous) { console.log(result); previous = result; }
    if (Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 250));
  } while (Date.now() < deadline);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch(() => {
    console.error('未完成连接读取。请确认已获授权的本机调试接口已启动，并检查端口、窗口焦点与 Termius 版本。');
    process.exitCode = 1;
  });
}
