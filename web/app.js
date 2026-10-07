'use strict';
const $ = id => document.getElementById(id);
const tokenStorageKey = 'pagecatch.accessToken';
function rememberedToken() {
  try { return localStorage.getItem(tokenStorageKey) || ''; }
  catch { return ''; }
}
function rememberToken(token) {
  try {
    if (token) localStorage.setItem(tokenStorageKey, token);
    else localStorage.removeItem(tokenStorageKey);
    return true;
  } catch { return false; }
}
const state = { tasks: [], info: null, downloadSettings: null, filter: 'all', token: rememberedToken(), headers: {}, epoch: 0, loading: true };
const statusNames = { pending: 'Queued', resolving: 'Resolving', downloading: 'Downloading', verifying: 'Verifying', ok: 'Completed', duplicate: 'Duplicate skipped', fail: 'Failed', cancelled: 'Cancelled' };
const isActive = task => ['pending', 'resolving', 'downloading', 'verifying'].includes(task.status);
const isDone = task => ['ok', 'duplicate'].includes(task.status);
function completionTime(timestamp) {
  if (!Number.isFinite(timestamp) || timestamp <= 0) return '';
  // Task timestamps are Unix seconds; show UTC+8 regardless of the browser timezone.
  const date = new Date((timestamp + 8 * 60 * 60) * 1000);
  return Number.isFinite(date.getTime()) ? date.toISOString().slice(0, 16).replace('T', '-') : '';
}
let refreshing = false, savingNetwork = false, savingDownloads = false, submitting = false, logVersion = 0, noticeVersion = 0, logTaskId = null;
let deleteTask = null, deletingRecord = false;
let readingFile = false, importVersion = 0;

function node(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}
function icon(name) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('class', 'icon');
  svg.setAttribute('aria-hidden', 'true');
  const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
  use.setAttribute('href', '#i-' + name);
  svg.append(use);
  return svg;
}
async function api(path, options = {}) {
  const { token = state.token, ...request } = options;
  const headers = { ...request.headers };
  if (token) headers.Authorization = 'Bearer ' + token;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 15000);
  try {
    const response = await fetch('/api' + path, { ...request, headers, cache: 'no-store', signal: controller.signal });
    if (!response.ok) {
      const body = await response.text();
      let message = body || 'Request failed: ' + response.status;
      try { message = JSON.parse(body).error || message; } catch { /* 非 JSON 错误由服务原文说明。 */ }
      const error = new Error(message);
      error.status = response.status;
      throw error;
    }
    return response;
  } catch (error) {
    if (error.name === 'AbortError') throw new Error('Request timed out. Check the server connection.');
    if (error instanceof TypeError) throw new Error('Cannot connect to the server. Check that it is running.');
    throw error;
  } finally { clearTimeout(timer); }
}
async function jsonApi(path, options) { return (await api(path, options)).json(); }
function message(id, text, error = false) {
  $(id).textContent = text;
  $(id).classList.toggle('error', error);
  $(id).setAttribute('role', error ? 'alert' : 'status');
}
function showNotice(text, error = false, kind = 'operation', auth = false) {
  const version = ++noticeVersion;
  $('notice').hidden = !text;
  $('notice').classList.toggle('error', error);
  $('notice').dataset.kind = kind;
  $('notice').setAttribute('role', error ? 'alert' : 'status');
  $('notice-text').textContent = text;
  $('notice-settings').hidden = !auth;
  if (text && !error) setTimeout(() => { if (version === noticeVersion) showNotice(''); }, 6000);
}
function setConnection(connected, needsToken = false) {
  $('connection').textContent = connected ? 'Connected' : needsToken ? 'Authentication required' : 'Disconnected';
  $('connection').classList.toggle('offline', !connected);
}
function setView(view) {
  const downloads = view === 'downloads';
  $('downloads-view').hidden = !downloads;
  $('settings-view').hidden = downloads;
  for (const [id, selected] of [['nav-downloads', downloads], ['nav-settings', !downloads]]) {
    if (selected) $(id).setAttribute('aria-current', 'page');
    else $(id).removeAttribute('aria-current');
  }
  document.title = 'PageCatch · ' + (downloads ? 'Downloads' : 'Settings');
}
function setSettingsTab(tab) {
  document.querySelectorAll('[data-setting]').forEach(button => {
    const selected = button.dataset.setting === tab;
    button.setAttribute('aria-selected', String(selected));
    button.tabIndex = selected ? 0 : -1;
    $('panel-' + button.dataset.setting).hidden = !selected;
  });
}
function renderInfo() {
  if (!state.info) return;
  const info = state.info;
  $('cpu_cores').max = info.cpu_available;
  if (!savingDownloads) {
    const dirty = downloadDirty();
    state.downloadSettings = { output: info.output, workers: info.workers, threads: info.threads, retries: info.retries, cpu_cores: info.cpu_cores };
    if (!dirty) fillDownloadSettings();
    updateDownloadButtons();
  }
  $('preview-row').hidden = !info.preview;
  $('preview').textContent = info.preview ? 'Preview: first ' + info.preview + ' segments' : 'Full download';
  $('version').textContent = 'PageCatch ' + info.version;
  $('sidebar-version').textContent = 'v' + info.version;
  $('browser').textContent = info.browser ? 'Chrome / Chromium available' : 'No browser found; static parsing only';
}
function restoreTaskFocus(id, action) {
  const buttons = [...$('task-list').querySelectorAll('button')];
  const target = buttons.find(b => b.dataset.task === String(id) && b.dataset.action === action) || buttons.find(b => b.dataset.task === String(id));
  (target || $('refresh')).focus({ preventScroll: true });
}
function renderTasks() {
  if ($('log-dialog').open || $('delete-dialog').open) return;
  const focused = document.activeElement;
  const focusKey = focused?.closest('#task-list button') ? { id: focused.dataset.task, action: focused.dataset.action } : null;
  const counts = { all: state.tasks.length, active: state.tasks.filter(isActive).length, done: state.tasks.filter(isDone).length, fail: state.tasks.filter(t => t.status === 'fail').length };
  $('nav-count').textContent = counts.all;
  for (const [name, count] of Object.entries(counts)) {
    $('count-' + name).textContent = count;
    $('count-' + name).hidden = name !== 'all' && count === 0;
  }
  const visible = state.tasks.filter(task => state.filter === 'all' || state.filter === 'active' && isActive(task) || state.filter === 'done' && isDone(task) || state.filter === 'fail' && task.status === 'fail');
  const list = $('task-list');
  const fragment = document.createDocumentFragment();
  if (!visible.length) {
    const empty = node('div', 'empty');
    empty.append(icon('download'), node('p', '', state.loading ? 'Loading tasks…' : state.filter === 'all' ? 'No tasks yet' : 'No matching tasks'));
    fragment.append(empty);
  }
  for (const task of visible) {
    const row = node('article', 'task');
    const fileIcon = node('div', 'file-icon'); fileIcon.append(icon('video'));
    const content = node('div', 'task-content');
    const name = node('div', 'task-name', task.name || 'video'); name.title = name.textContent;
    const source = node('div', 'task-url', task.url); source.title = task.url;
    content.append(name, source);
    if (task.message && task.message !== statusNames[task.status] && !['ok', 'duplicate'].includes(task.status)) content.append(node('p', 'task-message' + (task.status === 'fail' ? ' error' : ''), task.message));
    const metadata = node('div', 'task-meta', '#' + task.id + ' · ' + (task.bytes / 1024 / 1024).toFixed(1) + ' MB');
    const completed = isDone(task) ? completionTime(task.updated) : '';
    if (completed) {
      const time = node('time', 'task-completed', completed);
      time.dateTime = new Date(task.updated * 1000).toISOString();
      time.title = 'Completed (UTC+8)';
      metadata.append(document.createTextNode(' · '), time);
    }
    if (task.attempt > 1) metadata.append(document.createTextNode(' · Retries: ' + (task.attempt - 1)));
    content.append(metadata);
    if (task.output) {
      const file = node('div', 'task-file'); file.title = task.output;
      file.append(icon('folder'), node('span', '', task.output.split(/[\\/]/).pop()));
      content.append(file);
    }
    const side = node('div', 'task-side');
    side.append(node('span', 'badge ' + task.status, statusNames[task.status] || task.status));
    const actions = node('div', 'task-actions');
    function action(label, symbol, type, callback) {
      const button = node('button', 'icon-button');
      button.type = 'button'; button.title = label; button.setAttribute('aria-label', label);
      button.dataset.task = String(task.id); button.dataset.action = type; button.append(icon(symbol));
      button.onclick = async () => {
        const hadFocus = document.activeElement === button;
        button.disabled = true;
        try { await callback(); }
        catch (error) { showNotice(error.message, true); }
        finally {
          button.disabled = false;
          if (hadFocus && document.activeElement === document.body && !$('log-dialog').open && !$('downloads-view').hidden) restoreTaskFocus(task.id, type);
        }
      };
      actions.append(button);
    }
    action('View logs', 'log', 'log', () => openLog(task));
    if (isActive(task)) action('Cancel task', 'close', 'cancel', async () => { await api('/tasks/' + task.id + '/cancel', { method: 'POST' }); await refresh(); });
    if (['fail', 'cancelled'].includes(task.status)) action('Retry task', 'retry', 'retry', async () => { await api('/tasks/' + task.id + '/retry', { method: 'POST' }); await refresh(); });
    if (task.output) action('Copy save path', 'copy', 'copy', async () => { await navigator.clipboard.writeText(task.output); showNotice('Save path copied'); });
    if (['ok', 'duplicate', 'fail', 'cancelled'].includes(task.status)) action('Delete record', 'trash', 'delete', () => openDeleteRecord(task));
    side.append(actions); row.append(fileIcon, content, side); fragment.append(row);
  }
  list.replaceChildren(fragment);
  if (focusKey) {
    const buttons = [...list.querySelectorAll('button')];
    const button = buttons.find(b => b.dataset.task === focusKey.id && b.dataset.action === focusKey.action) || buttons.find(b => b.dataset.task === focusKey.id);
    button?.focus({ preventScroll: true });
  }
}
async function refresh() {
  if (refreshing) return;
  refreshing = true; $('refresh').setAttribute('aria-busy', 'true');
  const epoch = state.epoch;
  try {
    const [tasks, info] = await Promise.all([jsonApi('/tasks'), jsonApi('/info')]);
    if (epoch !== state.epoch) return;
    state.tasks = tasks; state.info = info; state.loading = false;
    setConnection(true);
    if ($('notice').dataset.kind === 'connection') showNotice('');
    renderTasks(); renderInfo();
  } catch (error) {
    if (epoch !== state.epoch) return;
    state.loading = false;
    setConnection(false, error.status === 401);
    showNotice(error.message, true, 'connection', error.status === 401);
    renderTasks();
  } finally { refreshing = false; $('refresh').removeAttribute('aria-busy'); }
}
function openDeleteRecord(task) {
  deleteTask = task;
  $('delete-name').textContent = task.name || 'video';
  message('delete-feedback', '');
  $('delete-dialog').showModal();
  $('cancel-delete').focus();
}
$('cancel-delete').onclick = () => $('delete-dialog').close();
$('delete-dialog').oncancel = event => { if (deletingRecord) event.preventDefault(); };
$('delete-dialog').onclose = () => {
  const id = deleteTask?.id;
  deleteTask = null;
  renderTasks();
  if (!$('downloads-view').hidden) restoreTaskFocus(id, 'delete');
};
$('delete-form').onsubmit = async event => {
  event.preventDefault();
  if (!deleteTask || deletingRecord) return;
  const id = deleteTask.id;
  deletingRecord = true;
  $('confirm-delete').disabled = $('cancel-delete').disabled = true;
  $('delete-form').setAttribute('aria-busy', 'true');
  message('delete-feedback', 'Deleting record…');
  try {
    await api('/tasks/' + id, { method: 'DELETE' });
    state.epoch++;
    state.tasks = state.tasks.filter(task => task.id !== id);
    $('delete-dialog').close();
    renderTasks();
    showNotice('Record deleted. Video files kept.');
    await refresh();
  } catch (error) { message('delete-feedback', error.message, true); }
  finally {
    deletingRecord = false;
    $('confirm-delete').disabled = $('cancel-delete').disabled = false;
    $('delete-form').setAttribute('aria-busy', 'false');
  }
};
function openAdd() {
  message('add-feedback', '');
  $('add-dialog').showModal();
}
function rejectionReason(item) {
  try { new URL(item.url); return item.reason; }
  catch { return 'Invalid URL. Enter a complete HTTP/HTTPS URL.'; }
}
function inputLines() { return PageCatchImport.extract($('urls').value).urls; }
function updateAddControls() {
  $('submit').disabled = submitting || readingFile;
  $('import-file').disabled = submitting || readingFile;
  $('urls').disabled = submitting || readingFile;
  $('add-form').setAttribute('aria-busy', String(submitting || readingFile));
}
$('urls').oninput = () => { const count = inputLines().length; $('url-count').textContent = count + ' / 200 URLs'; $('url-count').classList.toggle('error', count > 200); };
$('urls').onpaste = event => {
  const text = event.clipboardData?.getData('text/plain');
  if (!text) return;
  if (new TextEncoder().encode(text).length > PageCatchImport.maxFileBytes) {
    event.preventDefault(); message('add-feedback', 'Text is too large. Paste up to 1 MB at a time.', true); return;
  }
  const result = PageCatchImport.extract(text);
  if (!result.urls.length) return;
  event.preventDefault();
  const input = $('urls');
  const combined = PageCatchImport.extract(input.value.slice(0, input.selectionStart) + '\n' + text + '\n' + input.value.slice(input.selectionEnd));
  input.value = combined.urls.join('\n'); input.oninput();
  input.setSelectionRange(input.value.length, input.value.length);
  message('add-feedback', 'Links extracted. Review before adding.' + (combined.duplicates ? ' Duplicates skipped: ' + combined.duplicates + '.' : ''));
};
$('import-file').onclick = () => { $('url-file').value = ''; $('url-file').click(); };
$('url-file').onchange = async () => {
  const file = $('url-file').files[0];
  if (!file || submitting || readingFile) return;
  const version = ++importVersion;
  readingFile = true; updateAddControls();
  message('add-feedback', 'Reading file…');
  try {
    const result = await PageCatchImport.readFile(file);
    if (version !== importVersion || !$('add-dialog').open) return;
    const merged = PageCatchImport.extract([...inputLines(), ...result.urls].join('\n'));
    $('urls').value = merged.urls.join('\n'); $('urls').oninput();
    const duplicates = result.duplicates + merged.duplicates;
    message('add-feedback', file.name + ': ' + result.urls.length + ' URLs found.' + (duplicates ? ' Duplicates skipped: ' + duplicates + '.' : '') + ' Review before adding.');
    if (merged.urls.length > 200) message('add-feedback', 'Found ' + merged.urls.length + ' unique URLs. Keep up to 200 for this batch; no links were dropped.', true);
  } catch (error) {
    if (version === importVersion && $('add-dialog').open) message('add-feedback', error.message, true);
  } finally {
    if (version === importVersion) { readingFile = false; updateAddControls(); $('url-file').value = ''; }
  }
};
$('add-dialog').onclose = () => { importVersion++; readingFile = false; $('url-file').value = ''; updateAddControls(); };
$('urls').onkeydown = event => { if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') { event.preventDefault(); $('add-form').requestSubmit(); } };
$('add-form').onsubmit = async event => {
  event.preventDefault();
  if (submitting || readingFile) return;
  const lines = inputLines();
  if (!lines.length || lines.length > 200) { message('add-feedback', 'Enter 1–200 valid HTTP/HTTPS URLs. You can paste chat history or import TXT / CSV.', true); return; }
  const body = JSON.stringify({ urls: lines.join('\n'), headers: state.headers });
  if (new TextEncoder().encode(body).length > 128 * 1024) { message('add-feedback', 'This batch exceeds the server request size. Submit fewer or shorter URLs.', true); return; }
  submitting = true; updateAddControls();
  message('add-feedback', 'Adding to queue…');
  try {
    const result = await jsonApi('/tasks', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body });
    const summary = 'Added ' + result.added.length + (result.added.length === 1 ? ' task' : ' tasks') + (result.skipped.length ? ', duplicates skipped: ' + result.skipped.length : '');
    if (result.rejected.length) {
      $('urls').value = result.rejected.map(item => item.url).join('\n'); $('urls').oninput();
      message('add-feedback', summary + '. ' + result.rejected.map(rejectionReason).join('; '), true);
    } else {
      $('urls').value = ''; $('urls').oninput(); $('add-dialog').close();
      showNotice(summary);
    }
    await refresh();
  } catch (error) { message('add-feedback', error.message, true); }
  finally {
    submitting = false; updateAddControls();
    if ($('add-dialog').open && document.activeElement === document.body) $('urls').focus();
  }
};
async function openLog(task) {
  const version = ++logVersion;
  logTaskId = task.id;
  $('log-title').textContent = 'Task Logs · #' + task.id;
  $('log-content').textContent = 'Loading logs…'; $('log-dialog').showModal();
  try {
    const response = await api('/tasks/' + task.id + '/log');
    const text = await response.text();
    if (version === logVersion && $('log-dialog').open) $('log-content').textContent = text;
  } catch (error) { if (version === logVersion && $('log-dialog').open) $('log-content').textContent = error.message; }
}
function headerText() { return Object.keys(state.headers).length ? JSON.stringify(state.headers, null, 2) : ''; }
function downloadDraft() {
  return { output: $('output').value.trim(), workers: Number($('workers').value), threads: Number($('threads').value), retries: Number($('retries').value), cpu_cores: Number($('cpu_cores').value) };
}
function downloadDirty() {
  if (!state.downloadSettings) return false;
  const draft = downloadDraft();
  return Object.keys(draft).some(key => draft[key] !== state.downloadSettings[key]);
}
function fillDownloadSettings() {
  if (!state.downloadSettings) return;
  for (const [key, value] of Object.entries(state.downloadSettings)) $(key).value = value;
  $('output').title = state.downloadSettings.output;
}
function updateDownloadButtons() {
  const disabled = savingDownloads || !state.downloadSettings;
  for (const id of ['output', 'workers', 'threads', 'retries']) $(id).disabled = disabled;
  $('cpu_cores').disabled = disabled || !state.info?.cpu_limit_supported;
  $('download-form').setAttribute('aria-busy', String(savingDownloads));
  $('save-download').disabled = disabled || !downloadDirty();
  $('reset-download').disabled = disabled || !downloadDirty();
}
for (const id of ['output', 'workers', 'threads', 'retries', 'cpu_cores']) $(id).oninput = () => { message('download-feedback', ''); updateDownloadButtons(); };
$('reset-download').onclick = () => { fillDownloadSettings(); message('download-feedback', ''); updateDownloadButtons(); $('output').focus(); };
$('download-form').onsubmit = async event => {
  event.preventDefault();
  if (savingDownloads || !state.downloadSettings) return;
  const cpuChanged = downloadDraft().cpu_cores !== state.downloadSettings.cpu_cores;
  savingDownloads = true; updateDownloadButtons(); message('download-feedback', 'Saving…');
  try {
    const settings = await jsonApi('/settings', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(downloadDraft()) });
    state.downloadSettings = settings; state.info = { ...state.info, ...settings }; state.epoch++;
    fillDownloadSettings();
    message('download-feedback', cpuChanged ? 'Saved. CPU limit applied; other settings apply to new tasks.' : 'Saved. Other settings apply to new tasks.');
    await refresh();
  } catch (error) { message('download-feedback', error.message, true); }
  finally {
    savingDownloads = false; updateDownloadButtons();
    if (!$('settings-view').hidden && !$('panel-download').hidden && document.activeElement === document.body) $('output').focus();
  }
};
function networkDirty() { return $('token').value.trim() !== state.token || $('headers').value !== headerText(); }
function updateNetworkButtons() {
  const dirty = networkDirty();
  $('token').disabled = savingNetwork;
  $('headers').disabled = savingNetwork;
  $('network-form').setAttribute('aria-busy', String(savingNetwork));
  $('save-network').disabled = savingNetwork || !dirty;
  $('reset-network').disabled = savingNetwork || !dirty;
}
function resetNetwork() { $('token').value = state.token; $('headers').value = headerText(); message('network-feedback', ''); updateNetworkButtons(); }
for (const id of ['token', 'headers']) $(id).oninput = () => { message('network-feedback', ''); updateNetworkButtons(); };
$('network-form').onsubmit = async event => {
  event.preventDefault();
  if (savingNetwork) return;
  let headers;
  try {
    headers = $('headers').value.trim() ? JSON.parse($('headers').value) : {};
    if (!headers || Array.isArray(headers) || typeof headers !== 'object' || Object.values(headers).some(value => typeof value !== 'string')) throw new Error('Headers must be a JSON object with string values.');
  } catch (error) { message('network-feedback', error.message, true); return; }
  const token = $('token').value.trim();
  savingNetwork = true; updateNetworkButtons(); message('network-feedback', 'Checking connection…');
  try {
    const info = await jsonApi('/info', { token });
    state.token = token; state.headers = headers; state.info = info; state.epoch++;
    $('token').value = state.token; $('headers').value = headerText();
    renderInfo(); setConnection(true);
    if ($('notice').dataset.kind === 'connection') showNotice('');
    const remembered = rememberToken(token);
    message('network-feedback', remembered ? 'Token remembered in this browser. Headers kept for this tab only.' : 'Connected, but this browser could not remember the token. Re-enter it after reloading.', !remembered);
    await refresh();
  } catch (error) { message('network-feedback', error.message, true); }
  finally {
    savingNetwork = false; updateNetworkButtons();
    if (!$('settings-view').hidden && !$('panel-network').hidden && document.activeElement === document.body) $('token').focus();
  }
};
$('reset-network').onclick = resetNetwork;
$('nav-downloads').onclick = () => setView('downloads');
$('nav-settings').onclick = () => setView('settings');
$('back-downloads').onclick = () => setView('downloads');
$('brand').onclick = event => { event.preventDefault(); setView('downloads'); };
$('notice-settings').onclick = () => { setView('settings'); setSettingsTab('network'); $('token').focus(); };
$('dismiss-notice').onclick = () => showNotice('');
$('collapse-nav').onclick = () => {
  const collapsed = $('app-shell').classList.toggle('is-collapsed');
  $('collapse-nav').setAttribute('aria-expanded', String(!collapsed));
  $('collapse-nav').setAttribute('aria-label', collapsed ? 'Expand sidebar' : 'Collapse sidebar');
  $('collapse-nav').title = collapsed ? 'Expand sidebar' : 'Collapse sidebar';
};
$('new-task').onclick = openAdd;
$('refresh').onclick = refresh;
document.querySelectorAll('[data-filter]').forEach(button => button.onclick = () => {
  state.filter = button.dataset.filter;
  document.querySelectorAll('[data-filter]').forEach(b => b.setAttribute('aria-pressed', String(b === button)));
  renderTasks();
});
const settingsTabs = [...document.querySelectorAll('[data-setting]')];
settingsTabs.forEach((button, index) => {
  button.onclick = () => setSettingsTab(button.dataset.setting);
  button.onkeydown = event => {
    let target;
    if (event.key === 'ArrowRight') target = settingsTabs[(index + 1) % settingsTabs.length];
    if (event.key === 'ArrowLeft') target = settingsTabs[(index - 1 + settingsTabs.length) % settingsTabs.length];
    if (event.key === 'Home') target = settingsTabs[0];
    if (event.key === 'End') target = settingsTabs[settingsTabs.length - 1];
    if (target) { event.preventDefault(); setSettingsTab(target.dataset.setting); target.focus(); }
  };
});
document.querySelectorAll('[data-close-dialog]').forEach(button => button.onclick = () => $(button.dataset.closeDialog).close());
for (const id of ['add-dialog', 'log-dialog']) {
  $(id).onclick = event => {
    const rect = $(id).getBoundingClientRect();
    if (event.target === $(id) && (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom)) $(id).close();
  };
}
$('log-dialog').onclose = () => { logVersion++; renderTasks(); restoreTaskFocus(logTaskId, 'log'); };
$('service-address').textContent = location.origin;
resetNetwork();
setInterval(refresh, 3000);
refresh();
