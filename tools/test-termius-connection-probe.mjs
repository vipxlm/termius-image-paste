import test from 'node:test';
import assert from 'node:assert/strict';
import { readFocusedConnection, validateEndpoint, validateSocket, projectObservation, probeTarget, inspectOnce } from './probe-termius-connection.mjs';

const endpoint = validateEndpoint('http://127.0.0.1:9222/');
const socketUrl = 'ws://127.0.0.1:9222/devtools/page/ABC-123';
function fixture({ host = '192.0.2.10', id = 'one', ready = true, focused = true, broadcast = false, props = {} } = {}) {
  const connection = { id: `connection-${id}`, protocol: 'ssh', host: { address: host }, get port() { return 2222; }, get username() { return 'tester'; } };
  for (const name of ['password', 'identity', 'privateKey']) Object.defineProperty(connection, name, { get() { throw new Error(`must not read ${name}`); } });
  const mediator = { terminalApi: { id }, currentConnection: connection, isShellReady: ready };
  const store = { getState: () => ({ terminal: { splitViewBroadcastTabs: new Set(broadcast ? ['workspace'] : []) } }) };
  const fiber = { memoizedProps: { mediator, ...props }, return: { memoizedProps: { value: { store } }, return: null } };
  const parent = { '__reactFiber$fixture': fiber, parentElement: null };
  const activeElement = { closest: selector => selector === '.xterm' ? {} : null, parentElement: parent };
  return { doc: { hasFocus: () => focused, activeElement }, mediator, connection, fiber };
}

test('uses focused pane connection fields and never touches authentication', () => {
  const { doc } = fixture();
  const result = readFocusedConnection(doc);
  assert.equal(result.status, 'connection-found');
  assert.equal(result.host, '192.0.2.10'); assert.equal(result.port, 2222); assert.equal(result.username, 'tester');
  assert.equal(result.broadcastChecked, true);
  assert(!JSON.stringify(result).includes('password'));
});
test('duplicate arbitrary titles do not affect focused pane selection', () => {
  const a = fixture({ id: 'a', host: '192.0.2.10', props: { title: 'same title' } });
  const b = fixture({ id: 'b', host: '192.0.2.20', props: { title: 'same title' } });
  assert.equal(readFocusedConnection(a.doc).host, '192.0.2.10');
  assert.equal(readFocusedConnection(b.doc).host, '192.0.2.20');
  a.connection.host.address = '192.0.2.30';
  assert.equal(readFocusedConnection(a.doc).host, '192.0.2.30');
});
test('rejects missing focus, unavailable connection, broadcast, and conflicting mediators', () => {
  assert.equal(readFocusedConnection(fixture({ focused: false }).doc).status, 'not-focused');
  const missing = fixture(); missing.doc.activeElement.closest = () => null;
  assert.equal(readFocusedConnection(missing.doc).status, 'terminal-not-focused');
  assert.equal(readFocusedConnection(fixture({ ready: false }).doc).status, 'shell-not-ready');
  assert.equal(readFocusedConnection(fixture({ broadcast: true }).doc).status, 'broadcast-active');
  const conflict = fixture(); conflict.fiber.return.return = { memoizedProps: { mediator: fixture({ id: 'other' }).mediator } };
  assert.equal(readFocusedConnection(conflict.doc).status, 'ambiguous-terminal');
  const unsupported = fixture(); unsupported.connection.protocol = 'local';
  assert.equal(readFocusedConnection(unsupported.doc).status, 'unsupported-protocol');
});
test('never invents a missing endpoint field and never projects extra fields', () => {
  const missing = fixture(); missing.connection.host.address = '';
  assert.equal(readFocusedConnection(missing.doc).status, 'incomplete-connection');
  const result = { ...readFocusedConnection(fixture().doc), password: 'must-be-excluded', unrelatedState: { key: 'must-be-excluded' } };
  assert(!JSON.stringify(projectObservation(result)).includes('must-be-excluded'));
  assert.deepEqual(projectObservation({ status: 'not-focused', password: 'secret' }), { status: 'not-focused' });
  assert.throws(() => projectObservation({ ...result, port: null }));
});
test('restricts HTTP and WebSocket to one explicit IPv4 loopback port', () => {
  for (const input of ['http://example.com:9222/', 'http://0.0.0.0:9222/', 'http://127.0.0.1:9222/?key=x', 'http://user:secret@127.0.0.1:9222/', 'https://127.0.0.1:9222/', 'http://127.0.0.1:9222/other']) assert.throws(() => validateEndpoint(input));
  assert.equal(validateSocket(socketUrl, endpoint), socketUrl);
  for (const input of ['ws://example.com:9222/devtools/page/ABC', 'ws://127.0.0.1:9223/devtools/page/ABC', 'ws://127.0.0.1:9222/other', 'ws://127.0.0.1:9222/devtools/page/ABC?token=secret']) assert.throws(() => validateSocket(input, endpoint));
});
class FakeSocket {
  static replies = [];
  constructor() { this.listeners = {}; queueMicrotask(() => this.emit('open', {})); }
  addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); }
  emit(name, event) { for (const fn of this.listeners[name] || []) fn(event); }
  send(text) {
    const request = JSON.parse(text);
    assert.equal(request.method, 'Runtime.evaluate'); assert.equal(request.params.throwOnSideEffect, true);
    assert.equal(request.params.returnByValue, true);
    const reply = FakeSocket.replies.shift();
    queueMicrotask(() => this.emit('message', { data: JSON.stringify({ id: 1, ...reply }) }));
  }
  close() { this.emit('close', {}); }
}
const reply = value => ({ result: { result: { value } } });
test('CDP read returns only selected fields and redacts raw renderer errors', async () => {
  FakeSocket.replies = [reply({ ...readFocusedConnection(fixture().doc), password: 'excluded' })];
  assert.equal((await probeTarget(socketUrl, FakeSocket)).host, '192.0.2.10');
  FakeSocket.replies = [{ result: { exceptionDetails: { text: 'sensitive raw renderer state' } } }];
  await assert.rejects(probeTarget(socketUrl, FakeSocket), error => !error.message.includes('sensitive'));
});
test('checks all UI windows and refuses ambiguous focused windows', async () => {
  const inventory = [1, 2].map(id => ({ type: 'page', url: `file:///app.asar/ui-process/src/ui-process/main.html?type=main&id=${id}#/tabs`, webSocketDebuggerUrl: socketUrl.replace('ABC-123', String(id)) }));
  const fetcher = async (url, options) => { assert.equal(url.href, 'http://127.0.0.1:9222/json/list'); assert.equal(options.redirect, 'error'); return { ok: true, text: async () => JSON.stringify(inventory) }; };
  FakeSocket.replies = [reply({ status: 'not-focused' }), reply(readFocusedConnection(fixture().doc))];
  assert.equal((await inspectOnce(endpoint, fetcher, FakeSocket)).status, 'connection-found');
  FakeSocket.replies = inventory.map(() => reply(readFocusedConnection(fixture().doc)));
  assert.equal((await inspectOnce(endpoint, fetcher, FakeSocket)).status, 'ambiguous-window');
});
