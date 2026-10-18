// Read-only Termius 10.2.1 adapter, shared by the native app and diagnostic CLI.
function readFocusedConnection(doc = document) {
  if (!doc.hasFocus()) return { status: 'not-focused' };
  const focused = doc.activeElement;
  // Element.closest is rejected by V8's side-effect guard in the shipped runtime.
  let inTerminal = false;
  for (let element = focused, depth = 0; element && depth < 64; element = element.parentElement, depth++) {
    if (typeof element.className === 'string' && element.className.split(/\s+/).includes('xterm')) { inTerminal = true; break; }
  }
  if (!inTerminal) return { status: 'terminal-not-focused' };
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
  // Resolve only the endpoint fields from the installed MergedSshConfig rules.
  // Do not call its constructors/getters: V8 conservatively rejects those calls.
  const entities = [];
  const seenEntities = new Set();
  for (let entity = connection.host; entity; entity = entities.length === 1 ? entity.group : entity.parent_group) {
    if (seenEntities.has(entity) || entities.length >= 64) return { status: 'incomplete-connection' };
    seenEntities.add(entity); entities.push(entity);
  }
  let port = 22; // Termius 10.2.1 MergedSshConfig's explicit global default.
  for (const entity of entities) {
    const value = entity.ssh_config?.port;
    if (value !== null && value !== undefined && value !== '') { port = value; break; }
  }
  let username = '';
  for (const entity of entities) {
    const identity = entity.ssh_config?.identity;
    if (!identity) continue;
    if (typeof identity.username === 'string' && identity.username.length > 0) { username = identity.username.trim(); break; }
    if (identity.is_visible) break;
    // Resolving an empty private identity may depend on credentials. Refuse it.
    return { status: 'incomplete-connection' };
  }
  if (!username) {
    const hostKey = connection.host.local_id != null ? `id:${connection.host.local_id}` : `addr:${host}:${port}`;
    const contextName = connection.perHostSSHidContexts?.[hostKey]?.username;
    if (typeof contextName === 'string') username = contextName;
  }
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
