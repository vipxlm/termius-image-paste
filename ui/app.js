'use strict';

(() => {
  const $ = id => document.getElementById(id);
  const tauri = window.__TAURI__;
  const bridgeAvailable = Boolean(tauri?.core?.invoke && tauri?.event?.listen);
  let state = null;
  let connected = false;
  let uiBusy = false;
  let currentPage = 'home';
  let editorProfile = null;
  let editorDirty = false;
  let trustCandidate = null;
  let useDefaultDirectory = true;
  let browsePath = null;
  let pendingLogin = false;
  let preferencesDraft = null;
  let preferencesDirty = false;
  let capturingShortcut = false;
  let toastTimer;
  let unlistenState;
  let unlistenDrop;
  let connecting = false;
  let serverListSignature = '';

  const fieldIds = {
    name: 'server-name', host: 'server-host', port: 'server-port',
    username: 'server-username', remoteDirectory: 'server-directory', keyFile: 'server-key'
  };
  const errorIds = { name: 'name-error', host: 'host-error', port: 'port-error', username: 'username-error', remoteDirectory: 'directory-error', keyFile: 'key-error' };
  const stageNames = {
    reading: '读取图片', clipboard: '读取图片', connecting: '连接服务器',
    uploading: '正在上传图片', upload: '正在上传图片', inserting: '正在输入图片路径',
    input: '正在输入图片路径', completed: '处理完成', complete: '处理完成',
    cancelled: '操作已取消', authorize: '正在授权服务器', authorizing: '正在授权服务器',
    testing: '正在测试连接', test: '正在测试连接', scanning: '正在读取主机指纹',
    scan: '正在读取主机指纹', fingerprint: '正在读取主机指纹',
    generating: '正在生成专用密钥', key: '正在准备专用密钥', cancelling: '正在取消操作'
  };

  function errorText(error) {
    if (typeof error === 'string') return error;
    if (error?.message) return [String(error.message), error.detail ? String(error.detail) : ''].filter(Boolean).join('\n\n');
    try { return JSON.stringify(error) || '操作未完成，请重试。'; }
    catch { return '操作未完成，请重试。'; }
  }

  function activeProfile() {
    const detected = state?.capabilities?.automaticTargetSelection ? state?.runtime?.detectedTarget : null;
    const id = detected ? detected.serverId : state?.settings?.activeServerId;
    return state?.settings?.servers?.find(profile => profile.id === id) || null;
  }

  function endpoint(profile) {
    if (!profile) return '';
    const host = String(profile.host || '');
    return `${profile.username || ''}@${host.includes(':') ? `[${host}]` : host}:${profile.port || 22}`;
  }

  function endpointIdentity(profile) {
    return `${String(profile.host || '').trim().toLowerCase()}\n${Number(profile.port)}`;
  }

  function targetText(target) {
    if (!target) return '';
    if (typeof target === 'string') return target;
    return target.host ? endpoint(target) : String(target.name || target.id || '');
  }

  function isRunning() {
    const operation = state?.runtime?.operation;
    return Boolean(operation && !['idle', 'success', 'error', 'warning', 'cancelled', 'completed', 'complete'].includes(operation.status));
  }

  function formatTime(value) {
    if (!value) return '';
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? String(value) : new Intl.DateTimeFormat('zh-CN', {
      month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hour12: false
    }).format(date);
  }

  function notice(id, message, kind = 'neutral') {
    const element = $(id);
    element.textContent = message;
    element.className = `notice ${kind}`;
    element.hidden = !message;
  }

  function toast(message) {
    $('toast').textContent = message;
    $('toast').hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { $('toast').hidden = true; }, 5500);
  }

  function showPage(name, focus = true) {
    currentPage = name;
    document.querySelectorAll('.page').forEach(page => { page.hidden = page.id !== `page-${name}`; });
    document.querySelectorAll('[data-page]').forEach(button => {
      if (button.dataset.page === name) button.setAttribute('aria-current', 'page');
      else button.removeAttribute('aria-current');
    });
    document.querySelector('main').scrollTop = 0;
    if (focus) $(`${name === 'home' ? 'home' : name === 'servers' ? 'servers' : 'settings'}-title`).focus({ preventScroll: true });
  }

  async function invoke(command, args = {}) {
    if (!bridgeAvailable) throw new Error('当前是浏览器预览，未连接本地应用，不能执行真实操作。');
    return tauri.core.invoke(command, args);
  }

  function setEditorStatus(message, kind = 'neutral', details = '') {
    notice('editor-status', message, kind);
    $('editor-details-section').hidden = !details;
    $('editor-details').textContent = details;
  }

  function renderAvailability() {
    const locked = !connected || uiBusy || isRunning();
    document.querySelectorAll('[data-app-action]').forEach(button => { button.disabled = locked; });
    const active = activeProfile();
    $('check-active').disabled = locked || !active;
    $('toggle-enabled').disabled = locked || !state?.settings || (!state.settings.enabled && !state.settings.servers.length);
    $('upload-clipboard').disabled = locked || !active || !state?.capabilities?.clipboardAvailable;
    $('upload-file').disabled = locked || !active;
    $('copy-path').disabled = locked || !state?.runtime?.lastResult?.path;
    $('server-fields').disabled = uiBusy || isRunning();
    $('cancel-edit').disabled = uiBusy || isRunning();
    $('shortcut-capture').disabled = !connected || uiBusy || isRunning();
    $('reset-shortcut').disabled = !connected || uiBusy || isRunning();
    ['legacy-shortcut', 'notifications'].forEach(id => { $(id).disabled = !connected || uiBusy || isRunning(); });
    $('close-to-tray').disabled = !connected || uiBusy || isRunning();
    const operation = state?.runtime?.operation;
    $('cancel-operation').disabled = !connected || !isRunning() || operation?.cancellable === false;
    if (editorProfile) {
      $('confirm-fingerprint').disabled = locked || !trustCandidate;
      $('browse-directory').disabled = locked || !editorProfile.verifiedAt;
      $('use-default-directory').disabled = uiBusy || isRunning();
    }
  }

  function shortcutParts(value) {
    return String(value || '').split('+').map(part => part.trim()).filter(Boolean).map(part => {
      if (part === 'Control') return 'Ctrl';
      if (part === 'Super' && String(state?.capabilities?.platform).toLowerCase() === 'windows') return 'Win';
      return part;
    });
  }

  function renderShortcut() {
    $('home-shortcut').replaceChildren();
    const parts = shortcutParts(state?.settings?.shortcut);
    if (parts.length === 0) $('home-shortcut').textContent = '尚未配置';
    parts.forEach(part => {
      const key = document.createElement('kbd');
      key.textContent = part;
      $('home-shortcut').append(key);
    });
    if (!capturingShortcut) $('shortcut-capture').textContent = shortcutParts(preferencesDraft?.shortcut).join(' + ') || '录入组合键';
    $('shortcut-capture').classList.toggle('capturing', capturingShortcut);
  }

  function renderRuntime() {
    const settings = state?.settings;
    const capabilities = state?.capabilities || {};
    const shortcutStatus = state?.runtime?.shortcutStatus;
    const shortcutMessage = typeof shortcutStatus === 'string' ? shortcutStatus : shortcutStatus?.message || shortcutStatus?.description || '';
    let label = '未连接本地应用';
    let badgeClass = '';
    if (connected && settings) {
      if (isRunning()) { label = '正在处理'; badgeClass = 'active'; }
      else if (!settings.enabled) label = '快捷键贴图已暂停';
      else if (!capabilities.automaticInput) { label = '手动上传模式'; badgeClass = 'warning'; }
      else if (!activeProfile()) { label = state?.runtime?.detectedTarget ? '当前会话尚未匹配' : '等待识别 Termius 会话'; badgeClass = 'warning'; }
      else if (shortcutMessage && /失败|冲突|占用|不可用|错误|error|unavailable|conflict|failed/i.test(shortcutMessage)) { label = '请检查快捷键'; badgeClass = 'warning'; }
      else { label = '快捷键贴图已启用'; badgeClass = 'active'; }
    }
    $('runtime-label').textContent = label;
    $('runtime-badge').className = `badge ${badgeClass}`;
    $('toggle-enabled').textContent = settings?.enabled ? '暂停贴图' : '启用贴图';
    $('shortcut-runtime').textContent = shortcutMessage || '快捷键注册结果由本地应用提供。';
    $('platform-label').textContent = connected ? `${capabilities.platform === 'windows' ? 'Windows' : capabilities.platform === 'linux' ? 'Linux' : capabilities.platform || '本机'}${capabilities.displayServer ? ` · ${capabilities.displayServer}` : ''}` : '未连接本地应用';
    $('save-target-help').textContent = capabilities.automaticTargetSelection ? '登录成功后保存。切到名称对应的 Termius 标签页，即可自动选择服务器。' : '登录成功后保存。新服务器需在列表中设为当前目标。';
    $('footer-status').textContent = connected ? '配置与 SSH 操作均在本机处理，密码不保存。' : '预览不可连接，不会执行上传、授权或保存。';
  }

  function renderHome() {
    const profile = activeProfile();
    $('current-name').textContent = profile?.name || '尚未选择服务器';
    $('current-endpoint').textContent = profile ? endpoint(profile) : '在“服务器”页面添加连接，并设为当前目标。';
    $('current-directory').textContent = profile?.remoteDirectory || '远程图片目录尚未配置';
    const detected = state?.capabilities?.automaticTargetSelection ? state?.runtime?.detectedTarget : null;
    if (detected) notice('home-notice', `${detected.message}（${detected.session || '未读取到标签名'}）`, profile ? 'success' : 'error');
    else if (!profile) notice('home-notice', state?.capabilities?.automaticTargetSelection ? '先添加服务器，再切到对应 Termius 标签页。会话名须与服务器名称或地址一致。' : '先添加服务器并登录。');
    else if (!profile.hostKey) notice('home-notice', '尚未信任此服务器的主机指纹。请编辑并核对后再上传。', 'error');
    else if (!profile.keyFile) notice('home-notice', '请编辑服务器，选择密码或密钥登录。');
    else if (profile.verifiedAt) notice('home-notice', `上次验证成功：${formatTime(profile.verifiedAt)}。请确认 Termius 当前会话也连接此服务器。`, 'success');
    else notice('home-notice', '此配置尚未验证。请编辑服务器，登录并保存。');
    const automatic = Boolean(state?.capabilities?.automaticInput);
    $('shortcut-help').textContent = automatic ? (state?.capabilities?.automaticTargetSelection ? '自动匹配当前 Termius 标签页。上传时保持该会话，等路径出现后再发送。' : '保持当前会话和输入框，等路径出现后再发送。') : '当前环境无法自动输入。请从此窗口上传，再自行复制路径。';
    $('manual-input-note').textContent = automatic ? '窗口内上传只显示路径。需要自动输入时，请回到 Termius 使用贴图快捷键。' : '当前环境使用手动上传模式。上传后可查看完整路径，需要时自行复制到 Termius。';
    $('manual-upload-help').textContent = state?.capabilities?.clipboardAvailable ? '也可以在此窗口按 Ctrl + V，或使用下面的按钮。' : '可在此窗口按 Ctrl + V 粘贴图片，或选择本机图片文件。';
  }

  function actionButton(text, onClick, extraClass = '') {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = text;
    button.dataset.appAction = '';
    if (extraClass) button.className = extraClass;
    button.addEventListener('click', onClick);
    return button;
  }

  function renderServers() {
    const signature = JSON.stringify([connected, activeProfile()?.id, state?.settings?.servers]);
    if (signature === serverListSignature) return;
    serverListSignature = signature;
    const list = $('server-list');
    list.replaceChildren();
    if (!state?.settings?.servers?.length) {
      const empty = document.createElement('p');
      empty.className = 'empty muted';
      empty.textContent = connected ? '还没有服务器。添加第一台连接后即可上传。' : '连接本地应用后显示已保存服务器。';
      list.append(empty);
      return;
    }
    state.settings.servers.forEach(profile => {
      const row = document.createElement('div');
      row.className = 'server-row';
      const info = document.createElement('div');
      info.className = 'server-info';
      const title = document.createElement('h3');
      title.textContent = profile.name;
      const address = document.createElement('p');
      address.className = 'mono muted';
      address.textContent = endpoint(profile);
      const validation = document.createElement('p');
      validation.className = 'small muted';
      validation.textContent = !profile.hostKey ? '尚未核对主机指纹' : profile.verifiedAt ? `上次验证：${formatTime(profile.verifiedAt)}` : '已信任主机，尚未验证连接';
      info.append(title, address, validation);
      const actions = document.createElement('div');
      actions.className = 'server-actions';
      if (profile.id === activeProfile()?.id) {
        const badge = document.createElement('span');
        badge.className = 'badge';
        badge.textContent = state?.runtime?.detectedTarget && state?.capabilities?.automaticTargetSelection ? '自动跟随' : '当前目标';
        actions.append(badge);
      } else actions.append(actionButton('设为当前目标', () => setActive(profile)));
      actions.append(actionButton('编辑服务器', () => openEditor(profile)));
      actions.append(actionButton('删除', () => removeProfile(profile), 'link danger-button'));
      row.append(info, actions);
      list.append(row);
    });
  }

  function renderResult() {
    const operation = state?.runtime?.operation;
    $('operation').hidden = !isRunning();
    if (isRunning()) {
      $('operation-title').textContent = stageNames[operation.stage] || '正在处理';
      $('operation-message').textContent = operation.message || '请稍候，操作结果将显示在下方。';
      $('operation-target').textContent = targetText(operation.target);
    }
    const result = state?.runtime?.lastResult;
    $('result-empty').hidden = Boolean(result);
    $('result-content').hidden = !result;
    if (!result) return;
    $('result-time').textContent = formatTime(result.at);
    $('result-status').textContent = result.status === 'success' ? '操作已完成' : result.status === 'error' ? '操作未完成' : result.path ? '图片已上传，未自动输入' : '请留意操作结果';
    $('result-summary').className = `notice ${result.status === 'success' ? 'success' : result.status === 'error' ? 'error' : ''}`;
    $('result-message').textContent = result.message || stageNames[result.stage] || '请查看详细信息。';
    $('result-target').textContent = targetText(result.target);
    $('result-path-section').hidden = !result.path;
    if ($('result-path').value !== (result.path || '')) $('result-path').value = result.path || '';
    $('result-details-section').hidden = !result.details;
    $('result-details').textContent = typeof result.details === 'string' ? result.details : result.details ? JSON.stringify(result.details, null, 2) : '';
  }

  function resetPreferencesFromState() {
    const settings = state?.settings;
    if (!settings) return;
    preferencesDraft = {
      shortcut: settings.shortcut || 'Ctrl+Shift+F12',
      legacyShortcutEnabled: Boolean(settings.legacyShortcutEnabled),
      closeToTray: Boolean(settings.closeToTray), notifications: Boolean(settings.notifications)
    };
    preferencesDirty = false;
    $('legacy-shortcut').checked = preferencesDraft.legacyShortcutEnabled;
    $('close-to-tray').checked = preferencesDraft.closeToTray;
    $('notifications').checked = preferencesDraft.notifications;
  }

  function renderCapabilities() {
    const capabilities = state?.capabilities || {};
    const items = [
      ['系统', `${capabilities.platform || '尚未读取'}${capabilities.displayServer ? ` · ${capabilities.displayServer}` : ''}`],
      ['自动输入', capabilities.automaticInput ? '可用，快捷键触发后仍会检查焦点' : '不可用，使用窗口上传并手动复制路径'],
      ['快捷键', capabilities.hotkeyAvailable ? '当前系统支持，实际注册状态见上方' : '当前环境不可用'],
      ['剪贴板图片', capabilities.clipboardAvailable ? '可后台读取，自动操作保留截图' : '无法后台读取，可在本窗口按 Ctrl + V 或选择图片文件'],
      ['托盘', capabilities.trayAvailable ? '可用，后台运行时可收起到托盘' : '不可用，后台运行时最小化到任务栏，窗口仍可找回']
    ];
    $('capability-list').replaceChildren();
    items.forEach(([label, value]) => {
      const row = document.createElement('div');
      const term = document.createElement('dt');
      const description = document.createElement('dd');
      term.textContent = label;
      description.textContent = value;
      row.append(term, description);
      $('capability-list').append(row);
    });
    notice('capability-description', capabilities.description || '实际能力由本地应用检测。没有自动输入能力时，仍可上传文件并查看路径。', capabilities.automaticInput ? 'neutral' : '');
    $('tray-description').textContent = capabilities.trayAvailable ? '继续接收贴图快捷键，可从托盘菜单退出。' : '托盘不可用时最小化到任务栏，不会将窗口隐藏到无法找回的位置。';
  }

  function applyState(next) {
    if (!next || !next.settings || !Array.isArray(next.settings.servers)) throw new Error('本地应用返回的状态不完整，请重新连接。');
    state = next;
    connected = true;
    $('connection-banner').hidden = true;
    if (!preferencesDraft || !preferencesDirty) resetPreferencesFromState();
    renderRuntime();
    renderHome();
    renderServers();
    renderResult();
    renderCapabilities();
    renderShortcut();
    $('settings-path').textContent = next.settingsPath || '本地应用未返回配置路径。';
    if (editorProfile) {
      $('active-editor-note').hidden = editorProfile.id !== activeProfile()?.id;
      renderEditorTrust();
    }
    renderAvailability();
  }

  function applySettings(settings) {
    if (!state) throw new Error('应用状态尚未加载。');
    applyState({ ...state, settings });
  }

  async function refreshState() {
    applyState(await invoke('get_state'));
  }

  async function perform(label, scope, action) {
    if (!connected || uiBusy || isRunning()) return false;
    uiBusy = true;
    if (scope === 'editor') setEditorStatus(label);
    else if (scope === 'preferences') notice('preferences-status', label);
    else if (scope === 'upload') notice('upload-error', '');
    else if (scope === 'servers') notice('servers-error', '');
    renderAvailability();
    try {
      await action();
      return true;
    } catch (error) {
      const message = errorText(error);
      if (scope === 'editor') setEditorStatus('操作未完成，请查看原因并修正后重试。', 'error', message);
      else if (scope === 'preferences') notice('preferences-status', message, 'error');
      else if (scope === 'upload') notice('upload-error', message, 'error');
      else if (scope === 'servers') notice('servers-error', message, 'error');
      else toast(message);
      return false;
    } finally {
      uiBusy = false;
      renderAvailability();
    }
  }

  function confirmAction(title, message, actionLabel = '继续') {
    const dialog = $('confirm-dialog');
    if (dialog.open) return Promise.resolve(false);
    $('confirm-title').textContent = title;
    $('confirm-message').textContent = message;
    $('confirm-action').textContent = actionLabel;
    dialog.returnValue = 'cancel';
    return new Promise(resolve => {
      dialog.addEventListener('close', () => resolve(dialog.returnValue === 'confirm'), { once: true });
      dialog.showModal();
    });
  }

  async function mayDiscardEditor() {
    return !editorDirty || await confirmAction('放弃未保存的修改？', '服务器修改尚未保存。放弃后会保留原有配置，当前上传目标不变。', '放弃修改');
  }

  async function openEditor(profile = null) {
    if (uiBusy || isRunning() || !await mayDiscardEditor()) return;
    editorProfile = profile ? { ...profile } : {
      id: makeId(), name: '', host: '', port: 22, username: '',
      keyFile: '', remoteDirectory: '', hostKey: null, verifiedAt: null
    };
    editorDirty = false;
    trustCandidate = null;
    pendingLogin = false;
    useDefaultDirectory = !profile?.remoteDirectory;
    $('login-method').value = profile?.keyFile ? 'key' : 'password';
    $('login-password').value = '';
    $('directory-browser').hidden = true;
    renderLoginMethod();
    Object.entries(fieldIds).forEach(([key, id]) => { $(id).value = editorProfile[key] ?? ''; });
    clearFieldErrors();
    $('editor-title').textContent = profile ? '编辑服务器' : '添加服务器';
    $('editor-dirty').textContent = '编辑不会切换当前上传目标';
    $('active-editor-note').hidden = !profile || profile.id !== activeProfile()?.id;
    $('fingerprint-review').hidden = true;
    setEditorStatus('');
    $('editor').hidden = false;
    showPage('servers', false);
    renderEditorTrust();
    renderAvailability();
    $('editor').scrollIntoView({ block: 'start', behavior: 'auto' });
    (profile ? $('editor-title') : $('server-host')).focus({ preventScroll: true });
  }

  function makeId() {
    if (crypto.randomUUID) return crypto.randomUUID().replaceAll('-', '');
    const bytes = crypto.getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    return Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
  }

  function readProfile() {
    if (!editorProfile) return null;
    const profile = { ...editorProfile };
    Object.entries(fieldIds).forEach(([key, id]) => { profile[key] = key === 'port' ? Number($(id).value.trim()) : $(id).value.trim(); });
    profile.name ||= profile.host;
    if (useDefaultDirectory) profile.remoteDirectory = '/';
    return profile;
  }

  function clearFieldErrors() {
    Object.entries(fieldIds).forEach(([key, id]) => {
      $(id).removeAttribute('aria-invalid');
      $(id).removeAttribute('aria-describedby');
      $(errorIds[key]).hidden = true;
    });
  }

  function validateProfile({ requireKey = false } = {}) {
    const profile = readProfile();
    if (!profile) return null;
    clearFieldErrors();
    const errors = {};
    const control = /[\u0000-\u001f\u007f-\u009f]/;
    if (!profile.name || profile.name.length > 128 || control.test(profile.name)) errors.name = '请填写名称，最多 128 个字符。';
    if (!profile.host || /^-/.test(profile.host) || /[\s\/@\[\]"'\\]/.test(profile.host) || control.test(profile.host)) errors.host = '请输入 DNS 名称、IPv4 或不带方括号的 IPv6 地址。';
    if (!/^\d+$/.test($('server-port').value.trim()) || !Number.isInteger(profile.port) || profile.port < 1 || profile.port > 65535) errors.port = '端口须为 1–65535 的整数。';
    if (!/^[A-Za-z_][A-Za-z0-9_.-]{0,63}\$?$/.test(profile.username)) errors.username = '请填写有效的 Linux 用户名。';
    if (!profile.remoteDirectory.startsWith('/') || control.test(profile.remoteDirectory)) errors.remoteDirectory = '请填写不含控制字符的 Linux 绝对路径。';
    if (control.test(profile.keyFile) || (requireKey && !profile.keyFile)) errors.keyFile = '请选择已获得服务器授权的私钥。';
    Object.entries(errors).forEach(([key, message]) => {
      const input = $(fieldIds[key]);
      input.setAttribute('aria-invalid', 'true');
      input.setAttribute('aria-describedby', errorIds[key]);
      $(errorIds[key]).textContent = message;
      $(errorIds[key]).hidden = false;
    });
    const first = Object.keys(errors)[0];
    if (first) { $(fieldIds[first]).focus(); return null; }
    return profile;
  }

  function markEditorDirty() {
    editorDirty = true;
    $('editor-dirty').textContent = '有未保存的修改';
  }

  function renderLoginMethod() {
    const password = $('login-method').value === 'password';
    $('password-field').hidden = !password;
    $('key-field').hidden = password;
    $('login-password').required = password;
  }

  function renderEditorTrust() {
    if (!editorProfile) return;
    $('browse-directory').disabled = !connected || uiBusy || isRunning() || !editorProfile.verifiedAt;
    $('directory-help').textContent = editorProfile.verifiedAt
      ? '已登录，可选择服务器上的其他可写文件夹。更改目录后会重新验证。'
      : '留空使用默认目录，登录成功后可浏览远端文件夹。';
  }

  function editorChanged(event) {
    if (!editorProfile) return;
    if (event.target.id === 'login-method') { $('login-password').value = ''; renderLoginMethod(); }
    if (event.target.id === 'server-directory') useDefaultDirectory = !event.target.value.trim();
    if (!Object.values(fieldIds).includes(event.target.id)) { markEditorDirty(); return; }
    const before = editorProfile;
    const after = readProfile();
    if (event.target.id === 'server-host' || event.target.id === 'server-port') {
      if (endpointIdentity(before) !== endpointIdentity(after)) {
        after.hostKey = null;
        trustCandidate = null;
        $('fingerprint-review').hidden = true;
      }
    }
    if (!['server-name', 'server-directory'].includes(event.target.id)) { after.verifiedAt = null; $('directory-browser').hidden = true; }
    editorProfile = after;
    markEditorDirty();
    renderEditorTrust();
    renderAvailability();
  }

  function confirmFingerprint() {
    const profile = readProfile();
    if (!profile || !trustCandidate || trustCandidate.endpoint !== endpointIdentity(profile)) return;
    editorProfile = { ...profile, hostKey: trustCandidate.key, verifiedAt: null };
    trustCandidate = null;
    $('fingerprint-review').hidden = true;
    markEditorDirty();
    if (pendingLogin) { pendingLogin = false; saveServer(); }
  }

  async function chooseKey() {
    await perform('请选择本机的 SSH 私钥文件。', 'editor', async () => {
      const path = await invoke('choose_key_file');
      if (!path) { setEditorStatus('未选择密钥文件，原配置保持不变。'); return; }
      $('server-key').value = path;
      editorProfile = { ...readProfile(), verifiedAt: null };
      markEditorDirty();
      renderEditorTrust();
      setEditorStatus('已选择私钥。点击“登录并保存”验证已有授权。');
    });
  }

  async function saveServer(event) {
    event?.preventDefault();
    const passwordLogin = $('login-method').value === 'password';
    const profile = validateProfile({ requireKey: !passwordLogin });
    if (!profile) return;
    if (passwordLogin) profile.keyFile = '';
    if (passwordLogin && !$('login-password').value) {
      setEditorStatus('请输入登录密码。', 'error'); $('login-password').focus(); return;
    }
    await perform('正在登录服务器…', 'editor', async () => {
      if (!profile.hostKey) {
        const candidate = await invoke('scan_host_key', { profile });
        if (!candidate?.key || !candidate?.fingerprint) throw new Error('无法读取服务器身份。');
        trustCandidate = { ...candidate, endpoint: endpointIdentity(profile) };
        pendingLogin = true;
        $('fingerprint-endpoint').textContent = endpoint(profile);
        $('fingerprint-value').textContent = candidate.fingerprint;
        $('fingerprint-review').hidden = false;
        setEditorStatus('确认指纹后自动继续登录。');
        return;
      }
      let password = passwordLogin ? $('login-password').value : null;
      $('login-password').value = '';
      let loggedIn;
      try {
        loggedIn = await invoke('login_server', { profile, password, useDefaultDirectory });
      } catch (error) {
        throw new Error(password ? errorText(error).split(password).join('[密码已隐藏]') : errorText(error));
      } finally { password = null; }
      editorProfile = loggedIn;
      $('server-key').value = loggedIn.keyFile;
      $('server-directory').value = loggedIn.remoteDirectory;
      $('server-name').value = loggedIn.name;
      useDefaultDirectory = false;
      // Reuse the newly authorized key if saving fails; do not ask for the password again.
      $('login-method').value = 'key';
      renderLoginMethod();
      const settings = await invoke('save_server', { profile: loggedIn });
      applySettings(settings);
      editorDirty = false;
      $('editor-dirty').textContent = '已登录并保存';
      setEditorStatus('登录成功，图片目录已就绪。服务器配置已保存。', 'success');
      renderEditorTrust();
      toast('服务器已登录并保存');
    });
  }

  async function browseDirectory(directory = null) {
    if (!editorProfile?.verifiedAt) return;
    await perform('正在读取远端目录…', 'editor', async () => {
      const paths = await invoke('browse_remote_directory', { profile: readProfile(), directory });
      browsePath = paths[0];
      $('browse-path').textContent = browsePath;
      $('directory-list').replaceChildren();
      paths.slice(1).sort().forEach(path => {
        const button = actionButton(path.split('/').pop() || '/', () => browseDirectory(path));
        $('directory-list').append(button);
      });
      if (paths.length === 1) $('directory-list').textContent = '没有子文件夹';
      $('directory-browser').hidden = false;
      setEditorStatus('选择文件夹后点击“使用此目录”。');
    });
  }

  async function setActive(profile) {
    if (editorDirty && editorProfile?.id === profile.id) {
      if (!await confirmAction('使用已保存的配置？', '此服务器还有未保存的编辑。本次切换使用已保存版本，不会把草稿作为上传目标。', '使用已保存配置')) return;
    }
    await perform('正在切换当前目标…', 'servers', async () => {
      applySettings(await invoke('set_active_server', { id: profile.id }));
      toast(`当前上传目标：${profile.name}`);
    });
  }

  async function removeProfile(profile) {
    const active = profile.id === activeProfile()?.id;
    const approved = await confirmAction('删除本地服务器配置？', `删除“${profile.name}”的本地配置？${active ? '\n这是当前上传目标，删除后请重新确认上传目标。' : ''}\n\n服务器上的图片、SSH 密钥与已有授权均会保留。`, '删除配置');
    if (!approved) return;
    await perform('正在删除本地配置…', 'servers', async () => {
      applySettings(await invoke('remove_server', { id: profile.id }));
      if (editorProfile?.id === profile.id) {
        editorProfile = null;
        editorDirty = false;
        $('editor').hidden = true;
      }
      toast('本地配置已删除，远端数据和授权均已保留');
    });
  }

  function markPreferencesDirty() {
    if (!preferencesDraft) return;
    preferencesDirty = true;
    preferencesDraft.legacyShortcutEnabled = $('legacy-shortcut').checked;
    preferencesDraft.closeToTray = $('close-to-tray').checked;
    preferencesDraft.notifications = $('notifications').checked;
    notice('preferences-status', '有未保存的偏好修改，当前仍使用原设置。');
  }

  function captureShortcut(event) {
    if (!capturingShortcut) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === 'Escape') { stopCapture(); return; }
    if (['Control', 'Alt', 'Shift', 'Meta', 'OS'].includes(event.key)) return;
    if (event.isComposing || event.key === 'Process' || event.key === 'Dead') {
      notice('preferences-status', '请结束输入法组合输入后，再按快捷键。', 'error');
      return;
    }
    let key = '';
    if (/^Key[A-Z]$/.test(event.code)) key = event.code.substring(3);
    else if (/^Digit[0-9]$/.test(event.code)) key = event.code.substring(5);
    else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(event.key)) key = event.key;
    else if ((!event.code || event.code === 'Unidentified') && /^[A-Za-z0-9]$/.test(event.key)) key = event.key.toUpperCase();
    if (!key || !(event.ctrlKey || event.altKey || event.shiftKey || event.metaKey)) {
      notice('preferences-status', '请选择 Ctrl、Alt、Shift 或 Super 与字母、数字、F1–F24 的组合。', 'error');
      return;
    }
    preferencesDraft.shortcut = [event.ctrlKey ? 'Ctrl' : '', event.altKey ? 'Alt' : '', event.shiftKey ? 'Shift' : '', event.metaKey ? 'Super' : '', key].filter(Boolean).join('+');
    markPreferencesDirty();
    stopCapture();
  }

  function stopCapture() {
    capturingShortcut = false;
    renderShortcut();
  }

  async function savePreferences(event) {
    event.preventDefault();
    if (!preferencesDraft) return;
    markPreferencesDirty();
    await perform('正在检查快捷键并保存偏好…', 'preferences', async () => {
      const settings = await invoke('save_preferences', { ...preferencesDraft });
      preferencesDirty = false;
      applySettings(settings);
      notice('preferences-status', '偏好已保存并应用。快捷键冲突时，原设置会继续保留。', 'success');
      toast('快捷键与偏好已保存');
    });
  }

  async function uploadClipboard() {
    if (!activeProfile()) { notice('upload-error', '请先选择当前上传目标。', 'error'); return; }
    showPage('home', false);
    await perform('正在读取剪贴板图片…', 'upload', async () => {
      await invoke('upload_clipboard');
      await refreshState();
    });
  }

  async function uploadFile(path = null) {
    if (!activeProfile()) { notice('upload-error', '请先选择当前上传目标。', 'error'); showPage('home'); return; }
    showPage('home', false);
    await perform('正在准备上传图片…', 'upload', async () => {
      const selectedPath = path || await invoke('choose_image_file');
      if (!selectedPath) return;
      await invoke('upload_file', { path: selectedPath });
      await refreshState();
    });
  }

  function readImageData(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.addEventListener('load', () => typeof reader.result === 'string' ? resolve(reader.result) : reject(new Error('无法读取这张图片，请选择图片文件。')), { once: true });
      reader.addEventListener('error', () => reject(new Error('读取剪贴板图片失败，请重新截图或选择图片文件。')), { once: true });
      reader.addEventListener('abort', () => reject(new Error('图片读取已取消。')), { once: true });
      reader.readAsDataURL(file);
    });
  }

  async function uploadPastedImage(file) {
    showPage('home', false);
    if (!activeProfile()) { notice('upload-error', '请先选择当前上传目标。', 'error'); return; }
    await perform('正在读取粘贴的图片…', 'upload', async () => {
      if (file.size > 32 * 1024 * 1024) throw new Error('图片不能超过 32 MiB，请缩小截图范围。');
      if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) throw new Error('目前支持 PNG、JPEG 和 WebP 图片，请选择其他图片文件。');
      const dataUrl = await readImageData(file);
      await invoke('upload_image_data', { dataUrl });
      await refreshState();
    });
  }

  async function copyPath() {
    const path = state?.runtime?.lastResult?.path;
    if (!path) return;
    if (!await confirmAction('复制图片路径', '复制路径会替换剪贴板中的截图。远程图片会保留。确认现在复制路径？', '复制路径')) return;
    await perform('正在复制路径…', 'upload', async () => {
      await invoke('copy_path', { path });
      toast('远程路径已复制到剪贴板');
    });
  }

  function showDisconnected(message, isPreview = false) {
    connected = false;
    $('connection-banner').hidden = false;
    $('connection-banner').className = `connection-banner notice ${isPreview ? '' : 'error'}`;
    $('connection-title').textContent = isPreview ? '浏览器预览，未连接本地应用' : '暂时无法连接本地应用';
    $('connection-message').textContent = message;
    $('retry-connection').hidden = isPreview;
    renderRuntime();
    renderAvailability();
  }

  async function connect() {
    if (connecting) return;
    if (!bridgeAvailable) {
      showDisconnected('请通过安装后的贴图助手打开。当前页面不能连接服务器、上传图片或保存配置，界面不会模拟成功。', true);
      return;
    }
    connecting = true;
    $('retry-connection').disabled = true;
    try {
      if (!unlistenState) unlistenState = await tauri.event.listen('app-state', event => {
        try { applyState(event.payload); }
        catch (error) { showDisconnected(errorText(error)); }
      });
      await refreshState();
      if (!unlistenDrop) {
        const currentWebview = tauri.webview?.getCurrentWebview?.();
        if (currentWebview?.onDragDropEvent) {
          unlistenDrop = await currentWebview.onDragDropEvent(event => {
            const payload = event.payload;
            $('upload-zone').classList.toggle('drag-over', payload.type === 'enter' || payload.type === 'over');
            if (payload.type !== 'drop') return;
            $('upload-zone').classList.remove('drag-over');
            const paths = payload.paths || [];
            if (paths.length !== 1) { toast('请每次拖入一张图片，避免上传到错误目标。'); return; }
            if (!uiBusy && !isRunning()) uploadFile(paths[0]);
            else toast('已有操作正在进行，请等待完成。');
          });
        } else {
          unlistenDrop = await tauri.event.listen('tauri://drag-drop', event => {
            const paths = event.payload?.paths || [];
            if (paths.length === 1 && !uiBusy && !isRunning()) uploadFile(paths[0]);
            else if (paths.length > 1) toast('请每次拖入一张图片。');
          });
        }
      }
    } catch (error) { showDisconnected(errorText(error)); }
    finally { connecting = false; $('retry-connection').disabled = false; }
  }

  document.querySelectorAll('[data-page]').forEach(button => button.addEventListener('click', () => { stopCapture(); showPage(button.dataset.page); }));
  $('manage-target').addEventListener('click', () => showPage('servers'));
  $('check-active').addEventListener('click', () => { const profile = activeProfile(); if (profile) openEditor(profile); });
  $('add-server').addEventListener('click', () => openEditor());
  $('cancel-edit').addEventListener('click', async () => {
    if (!await mayDiscardEditor()) return;
    editorProfile = null;
    $('login-password').value = '';
    pendingLogin = false;
    editorDirty = false;
    $('editor').hidden = true;
    $('add-server').focus();
  });
  $('server-form').addEventListener('input', editorChanged);
  $('server-form').addEventListener('submit', saveServer);
  $('confirm-fingerprint').addEventListener('click', confirmFingerprint);
  $('reject-fingerprint').addEventListener('click', () => {
    trustCandidate = null; pendingLogin = false; $('login-password').value = '';
    $('fingerprint-review').hidden = true; setEditorStatus('已取消连接。');
  });
  $('choose-key').addEventListener('click', chooseKey);
  $('browse-directory').addEventListener('click', () => browseDirectory(useDefaultDirectory ? null : $('server-directory').value || null));
  $('browse-parent').addEventListener('click', () => browseDirectory(browsePath?.replace(/\/[^/]+\/?$/, '') || '/'));
  $('select-directory').addEventListener('click', () => {
    $('server-directory').value = browsePath; useDefaultDirectory = false;
    editorProfile = readProfile(); markEditorDirty(); $('directory-browser').hidden = true;
    setEditorStatus('已选择目录，点击“登录并保存”验证并应用。');
  });
  $('close-browser').addEventListener('click', () => { $('directory-browser').hidden = true; });
  $('use-default-directory').addEventListener('click', () => {
    $('server-directory').value = ''; useDefaultDirectory = true; markEditorDirty();
    $('directory-browser').hidden = true; setEditorStatus('保存时使用服务器用户主目录下的默认目录。');
  });
  $('preferences-form').addEventListener('submit', savePreferences);
  ['legacy-shortcut', 'close-to-tray', 'notifications'].forEach(id => $(id).addEventListener('change', markPreferencesDirty));
  $('shortcut-capture').addEventListener('click', () => {
    if (!preferencesDraft || !connected) return;
    capturingShortcut = true;
    $('shortcut-capture').textContent = '请按组合键，Esc 取消';
    $('shortcut-capture').classList.add('capturing');
  });
  $('shortcut-capture').addEventListener('keydown', captureShortcut);
  $('shortcut-capture').addEventListener('blur', stopCapture);
  $('reset-shortcut').addEventListener('click', () => {
    if (!preferencesDraft) return;
    preferencesDraft.shortcut = 'Ctrl+Shift+F12';
    markPreferencesDirty();
    stopCapture();
  });
  $('toggle-enabled').addEventListener('click', () => perform('正在切换贴图状态…', 'upload', async () => {
    const settings = await invoke('set_enabled', { enabled: !state.settings.enabled });
    applySettings(settings);
    toast(settings.enabled ? '快捷键贴图已启用' : '快捷键贴图已暂停');
  }));
  $('upload-clipboard').addEventListener('click', uploadClipboard);
  $('upload-file').addEventListener('click', () => uploadFile());
  $('copy-path').addEventListener('click', copyPath);
  $('cancel-operation').addEventListener('click', async () => {
    $('cancel-operation').disabled = true;
    try { await invoke('cancel_operation'); await refreshState(); }
    catch (error) { notice('upload-error', errorText(error), 'error'); }
    finally { renderAvailability(); }
  });
  $('retry-connection').addEventListener('click', connect);
  document.addEventListener('paste', event => {
    if (currentPage !== 'home' || event.target.closest?.('input,textarea,[contenteditable="true"]') || !connected || uiBusy || isRunning()) return;
    const image = Array.from(event.clipboardData?.files || []).find(file => file.type.startsWith('image/'));
    if (image) { event.preventDefault(); uploadPastedImage(image); return; }
    const hasImage = Array.from(event.clipboardData?.items || []).some(item => item.type.startsWith('image/'));
    if (hasImage) { event.preventDefault(); uploadClipboard(); }
  });
  document.addEventListener('dragover', event => { event.preventDefault(); });
  document.addEventListener('drop', event => {
    event.preventDefault();
    if (!bridgeAvailable) toast('浏览器预览不能读取本机文件路径，请打开桌面应用。');
  });
  window.addEventListener('beforeunload', () => {
    if (unlistenState) unlistenState();
    if (unlistenDrop) unlistenDrop();
    $('login-password').value = '';
  });
  renderAvailability();
  connect();
})();
