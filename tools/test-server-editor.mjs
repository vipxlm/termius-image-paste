import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';

// Exercise the actual editor script with an isolated DOM and IPC boundary.
// No SSH connection, credentials, or configuration files are used.
const html = fs.readFileSync(new URL('../ui/index.html', import.meta.url), 'utf8');
const nodes = new Map();
class Element {
  constructor(id = '') { this.id = id; this.value = ''; this.hidden = false; this.dataset = {}; this.children = []; this.listeners = {}; this.classList = { toggle() {}, remove() {} }; }
  addEventListener(name, fn) { (this.listeners[name] ||= []).push(fn); }
  async fire(name) { for (const fn of this.listeners[name] || []) await fn({ target: this, preventDefault() {} }); }
  append(...items) { this.children.push(...items); }
  replaceChildren(...items) { this.children = items; }
  setAttribute() {} removeAttribute() {} focus() {} scrollIntoView() {}
}
for (const match of html.matchAll(/id="([^"]+)"/g)) { assert(!nodes.has(match[1]), `duplicate id: ${match[1]}`); nodes.set(match[1], new Element(match[1])); }
const get = id => { assert(nodes.has(id), `missing element: ${id}`); return nodes.get(id); };
const main = new Element();
const documentEvents = {};
const document = { getElementById: get, querySelector: () => main, querySelectorAll: () => [], createElement: () => new Element(), addEventListener(name, fn) { documentEvents[name] = fn; } };
let saved = null;
let failLogin = false;
const calls = [];
let stateListener;
let dragDropListener;
let capabilities = {};
const settings = { servers: [], activeServerId: null, shortcut: 'Ctrl+Shift+F12' };
const invoke = async (command, args) => {
  calls.push({ command, args: structuredClone(args) });
  if (command === 'get_state') return { settings, runtime: {}, capabilities };
  if (command === 'choose_upload_file') return 'C:/isolated/报告.pdf';
  if (['upload_file', 'upload_clipboard', 'upload_file_data', 'upload_image_data'].includes(command)) return;
  if (command === 'scan_host_key') return { key: 'test-public-key', fingerprint: 'SHA256:test' };
  if (command === 'login_server') {
    if (failLogin) throw new Error(`denied ${args.password}`);
    return { ...args.profile, keyFile: 'C:/isolated/key', remoteDirectory: args.useDefaultDirectory ? '/srv/account/.cache/termius-images' : args.profile.remoteDirectory, verifiedAt: '2026-01-01T00:00:00Z' };
  }
  if (command === 'save_server') { saved = args.profile; settings.servers = [saved]; return settings; }
  if (command === 'browse_remote_directory') return [args.directory || '/srv/account', '/srv/account/photos'];
  throw new Error(`unexpected command ${command}`);
};
vm.runInNewContext(fs.readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8'), {
  document, window: { __TAURI__: { core: { invoke }, event: { listen: async (name, callback) => { if (name === 'app-state') stateListener = callback; if (name === 'tauri://drag-drop') dragDropListener = callback; return () => {}; } } }, addEventListener() {} },
  FileReader: class {
    listeners = {};
    addEventListener(name, fn) { this.listeners[name] = fn; }
    readAsDataURL(file) { this.result = file.dataUrl; this.listeners.load(); }
  },
  crypto: webcrypto, Intl, setTimeout: () => 1, clearTimeout() {}, console
});
const settle = async () => { for (let i = 0; i < 25; i++) await Promise.resolve(); };
await settle();
await get('add-server').fire('click');
get('server-host').value = 'example.test';
get('server-port').value = '22';
get('server-username').value = 'account';
get('login-password').value = 'isolated-secret';
await get('server-form').fire('submit');
assert.equal(calls.filter(c => c.command === 'login_server').length, 0, 'must wait for fingerprint approval');
assert.equal(get('fingerprint-review').hidden, false);
await get('confirm-fingerprint').fire('click');
await settle();
assert.equal(saved.name, 'example.test', 'optional name falls back to host');
assert.equal(saved.remoteDirectory, '/srv/account/.cache/termius-images');
assert.equal(get('login-password').value, '', 'password cleared after login');
assert(!('password' in saved), 'password must never enter saved profile');
assert.equal(get('login-method').value, 'key');
await get('browse-directory').fire('click');
assert.equal(get('directory-browser').hidden, false);
assert.equal(get('directory-list').children.length, 1);
await get('select-directory').fire('click');
await get('server-form').fire('submit');
assert.equal(calls.filter(c => c.command === 'login_server').at(-1).args.password, null, 'directory change reuses authorized key');
assert.equal(calls.filter(c => c.command === 'login_server').at(-1).args.useDefaultDirectory, false);
get('server-host').value = 'other.test';
for (const fn of get('server-form').listeners.input) fn({ target: get('server-host') });
assert.equal(get('browse-directory').disabled, true, 'new endpoint must invalidate verified session');
await get('server-form').fire('submit');
assert.equal(get('fingerprint-review').hidden, false, 'new endpoint requires new trust');
await get('reject-fingerprint').fire('click');
get('server-host').value = 'example.test';
for (const fn of get('server-form').listeners.input) fn({ target: get('server-host') });
get('login-method').value = 'password';
for (const fn of get('server-form').listeners.input) fn({ target: get('login-method') });
get('login-password').value = 'failed-secret';
failLogin = true;
const savesBefore = calls.filter(c => c.command === 'save_server').length;
await get('server-form').fire('submit');
await get('confirm-fingerprint').fire('click');
await settle();
assert.equal(calls.filter(c => c.command === 'save_server').length, savesBefore, 'failed login cannot save');
assert.equal(get('login-password').value, '');
assert(get('editor-details').textContent.includes('[密码已隐藏]'), 'login error must be surfaced');
assert(!get('editor-details').textContent.includes('failed-secret'), 'errors must redact password');
console.log('服务器编辑器：指纹确认、密码清除与脱敏、默认目录、目录选择、密钥复用、主机变更与失败不保存，全部通过。');

const another = { ...saved, id: 'another', name: 'Another', host: 'other.example.test' };
settings.servers = [saved, another];
stateListener({ payload: { settings, capabilities: { automaticInput: true, automaticTargetSelection: true }, runtime: { detectedTarget: { session: 'user@192.0.2.20:22', serverId: another.id, message: '已跟随' } } } });
assert.equal(get('current-name').textContent, 'Another', 'overview follows detected server instead of saved manual target');
assert(get('current-endpoint').textContent.includes('other.example.test'));
stateListener({ payload: { settings, capabilities: { automaticInput: true, automaticTargetSelection: true }, runtime: { detectedTarget: { session: 'user@192.0.2.30:22', serverId: null, message: '无法匹配' } } } });
assert.equal(get('upload-file').disabled, true, 'unknown session cannot fall back to previous server');
assert(get('home-notice').textContent.includes('无法匹配'));
console.log('自动目标界面：目标跟随与无匹配时阻止上传，通过。');

// Exercise file entry points through the same DOM/IPC fixture.
settings.activeServerId = saved.id;
capabilities = { platform: 'Windows', clipboardAvailable: true };
stateListener({ payload: { settings, capabilities, runtime: {} } });
await get('upload-file').fire('click');
assert.equal(calls.filter(c => c.command === 'upload_file').at(-1).args.path, 'C:/isolated/报告.pdf');
dragDropListener({ payload: { paths: ['C:/isolated/archive.zip'] } });
await settle();
assert.equal(calls.filter(c => c.command === 'upload_file').at(-1).args.path, 'C:/isolated/archive.zip');
const countUploads = () => calls.filter(c => c.command.startsWith('upload_')).length;
const beforeMultipleDrop = countUploads();
dragDropListener({ payload: { paths: ['a.pdf', 'b.pdf'] } });
await settle();
assert.equal(countUploads(), beforeMultipleDrop, 'multiple drop must not silently upload first file');
function paste(files, target = {}) {
  let prevented = false;
  documentEvents.paste({ target, clipboardData: { files }, preventDefault() { prevented = true; } });
  return prevented;
}
assert(paste([{ name: '报告.pdf', type: 'application/pdf', size: 10 }]));
await settle();
assert.equal(calls.filter(c => c.command.startsWith('upload_')).at(-1).command, 'upload_clipboard');
const beforeMultiplePaste = countUploads();
assert(paste([{ name: 'a.pdf' }, { name: 'b.pdf' }]));
await settle();
assert.equal(countUploads(), beforeMultiplePaste, 'multiple paste must not upload first file');
assert.equal(paste([{ name: 'a.pdf' }], { closest: () => true }), false, 'input fields retain ordinary paste');
capabilities = { platform: 'Linux' };
stateListener({ payload: { settings, capabilities, runtime: {} } });
const file = { name: 'binary.zip', type: 'application/zip', size: 3, dataUrl: 'data:application/zip;base64,AP8B' };
assert(paste([file]));
await settle();
assert.deepEqual(calls.filter(c => c.command === 'upload_file_data').at(-1).args, { name: file.name, dataUrl: file.dataUrl });
const beforeLarge = countUploads();
paste([{ ...file, size: 33 * 1024 * 1024 }]);
await settle();
assert.equal(countUploads(), beforeLarge, 'oversized web paste must fail before IPC');
assert(paste([{ name: 'image.png', type: 'image/png', size: 3, dataUrl: 'data:image/png;base64,AAAA' }]));
await settle();
assert.equal(calls.filter(c => c.command.startsWith('upload_')).at(-1).command, 'upload_image_data', 'image paste retains decoder');
console.log('文件上传：选择 PDF、拖入压缩包、Windows 原生粘贴、普通文件数据粘贴、多文件拒绝、输入框粘贴保留、大小限制和截图回归，全部通过。');
