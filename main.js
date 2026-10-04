'use strict';

const {
  app, Tray, Menu, BrowserWindow, ipcMain, nativeImage,
  Notification, screen, clipboard,
} = require('electron');
const fs = require('fs');
const path = require('path');
const { snapshot } = require('./data');

// Windows：通知/任务栏归属需要稳定的 AppUserModelID（须在 ready 前设置）
if (process.platform === 'win32') {
  app.setAppUserModelId('com.galaxrin.mimo-dash');
}

const { FIELDS, computeValues, isRed } = require('./fields');

// ---------- 配置 ----------
const DEFAULT_CONFIG = {
  refreshSec: 60,
  launchAtLogin: false,
  float: {
    show: true,
    opacity: 0.96,
    accent: '#4c8dff',
    density: 'std',
    mode: 'full', // full | mini
    heroIds: [null, null], // 核心位；null = 自动取前两个已勾选字段
    bounds: null, // 位置记忆
    fields: [
      { id: 'balance.total', on: true },
      { id: 'usage.monthPercent', on: true },
      { id: 'plan.name', on: true },
      { id: 'plan.periodEnd', on: true },
    ],
  },
  tray: { showTitle: true, fields: [
    { id: 'balance.total', on: true },
    { id: 'usage.monthPercent', on: true },
    { id: 'plan.name', on: true },
    { id: 'plan.periodEnd', on: true },
  ] },
  thresholds: {},
};

function mergeFieldList(list) {
  const arr = Array.isArray(list) ? list.map((f) => ({ ...f })) : [];
  const known = new Set(arr.map((f) => f.id));
  for (const m of FIELDS) {
    if (!known.has(m.id)) arr.push({ id: m.id, on: false });
  }
  return arr.filter((f) => FIELDS.some((m) => m.id === f.id));
}

function configPath() {
  return path.join(app.getPath('userData'), 'config.json');
}

function loadConfig() {
  let raw = {};
  const p = configPath();
  if (fs.existsSync(p)) {
    try {
      // 容忍 UTF-8 BOM（外部工具写入），解析失败回退默认并提示而不是静默重置
      const text = fs.readFileSync(p, 'utf8');
      // 去掉 UTF-8 BOM（外部工具写入时可能出现）
      raw = JSON.parse(text.charCodeAt(0) === 0xfeff ? text.slice(1) : text);
    } catch (e) {
      console.warn('[config] config.json 解析失败，使用默认配置:', e.message);
      raw = {};
    }
  }
  // 迁移：旧版单一 fields 列表 → 悬浮窗/托盘两份
  const legacyFields = Array.isArray(raw.fields) ? raw.fields : null;
  return {
    ...DEFAULT_CONFIG,
    ...raw,
    fields: undefined, // 旧键废弃，不再持久化
    refreshSec: raw.refreshSec ?? DEFAULT_CONFIG.refreshSec,
    launchAtLogin: raw.launchAtLogin ?? false,
    thresholds: raw.thresholds || {},
    tray: {
      ...DEFAULT_CONFIG.tray,
      ...(raw.tray || {}),
      fields: mergeFieldList(
        (raw.tray && raw.tray.fields) || legacyFields || DEFAULT_CONFIG.tray.fields
      ),
    },
    float: {
      ...DEFAULT_CONFIG.float,
      ...(raw.float || {}),
      fields: mergeFieldList(
        (raw.float && raw.float.fields) || legacyFields || DEFAULT_CONFIG.float.fields
      ),
      scale: undefined, // 弃用：改窗口拉伸，缩放记忆走 bounds
      accent: undefined, // 弃用：主题色选项移除，固定默认色
    },
  };
}

let config = loadConfig();
function saveConfig() {
  fs.writeFileSync(configPath(), JSON.stringify(config, null, 2));
}
let appQuitting = false;

// ---------- 余额历史（本地，供 sparkline） ----------
const historyPath = () => path.join(app.getPath('userData'), 'history.json');
let balanceHistory = {}; // { 'YYYY-MM-DD': number }
try {
  balanceHistory = JSON.parse(fs.readFileSync(historyPath(), 'utf8'));
} catch {}

function recordHistory(balanceStr) {
  const v = parseFloat(balanceStr);
  if (Number.isNaN(v)) return;
  const day = new Date().toISOString().slice(0, 10);
  if (balanceHistory[day] === v) return;
  balanceHistory[day] = v;
  const days = Object.keys(balanceHistory).sort();
  while (days.length > 30) delete balanceHistory[days.shift()];
  fs.writeFileSync(historyPath(), JSON.stringify(balanceHistory));
}

function history7() {
  return Object.entries(balanceHistory)
    .sort(([a], [b]) => a.localeCompare(b))
    .slice(-7)
    .map(([d, v]) => ({ d, v }));
}

// ---------- 阈值告警（新变红 → 系统通知，30 分钟限频） ----------
const prevRed = new Set();
const lastNotifyAt = new Map();

function checkThresholdAlerts() {
  if (!data) return;
  const values = computeValues(data);
  const now = Date.now();
  for (const id of Object.keys(config.thresholds || {})) {
    const red = isRed(id, values[id], config.thresholds);
    if (red && !prevRed.has(id)) {
      const last = lastNotifyAt.get(id) || 0;
      if (now - last > 30 * 60 * 1000 && Notification.isSupported()) {
        lastNotifyAt.set(id, now);
        const label = (FIELDS.find((m) => m.id === id) || {}).label || id;
        console.log(`[alert] ${label} -> ${values[id] ? values[id].text : '?'}`);
        new Notification({
          title: `MiMo 仪表盘 · ${label}触发阈值`,
          body: `当前 ${values[id] ? values[id].text : '—'}，请关注`,
        }).show();
      }
    }
    if (red) prevRed.add(id);
    else prevRed.delete(id);
  }
}

// ---------- 运行时状态 ----------
let data = null;
let errorMsg = null;
let tray = null;
let floatWin = null;
let settingsWin = null;
let timer = null;

function orderedEntries(target = 'float') {
  const values = data ? computeValues(data) : {};
  const list = target === 'tray' ? config.tray.fields : config.float.fields;
  return (list || [])
    .filter((f) => f.on && values[f.id])
    .map((f) => ({
      id: f.id,
      label: (FIELDS.find((m) => m.id === f.id) || {}).label || f.id,
      text: values[f.id].text,
      num: values[f.id].num,
      kind: (FIELDS.find((m) => m.id === f.id) || {}).kind || 'text',
      red: isRed(f.id, values[f.id], config.thresholds),
    }));
}

function publicState() {
  return {
    entries: orderedEntries('float'),
    trayEntries: orderedEntries('tray'),
    history: history7(),
    error: errorMsg,
    ts: data ? data.ts : 0,
    config,
    fieldsMeta: FIELDS,
  };
}

function broadcast() {
  for (const win of [floatWin, settingsWin]) {
    if (win && !win.isDestroyed()) win.webContents.send('state', publicState());
  }
  updateTray();
}

async function refresh() {
  try {
    data = await snapshot();
    errorMsg = null;
    if (data.balance) recordHistory(data.balance.balance);
    checkThresholdAlerts();
  } catch (e) {
    errorMsg = String(e && e.message ? e.message : e);
  }
  broadcast();
}

function startTimer() {
  if (timer) clearInterval(timer);
  timer = setInterval(refresh, Math.max(10, config.refreshSec) * 1000);
}

// ---------- 托盘 ----------
function traySummary() {
  // 全部已勾选字段都展示，不设上限
  return orderedEntries('tray').map((e) => e.text).join(' · ');
}

function updateTray() {
  if (!tray) return;
  const summary = config.tray.showTitle ? traySummary() : '';
  // 菜单栏文字摘要只在 macOS 有对应物；Windows 托盘没有文字位
  if (process.platform === 'darwin') {
    tray.setTitle(summary ? ` ${summary}` : '');
  }
  // 悬停 tooltip：各平台都带上摘要，Windows 上这是主要的“扫一眼”入口
  const tip = summary ? `MiMo 仪表盘 · ${summary}` : 'MiMo 仪表盘';
  tray.setToolTip(tip.length > 120 ? `${tip.slice(0, 119)}…` : tip);
  // 下拉：设置 + 退出；Windows 额外把已勾选字段列成菜单项（替代托盘文字）
  const entries = process.platform !== 'darwin' && summary
    ? orderedEntries('tray').map((e) => ({ label: `${e.label}  ${e.text}`, enabled: false }))
    : [];
  tray.setContextMenu(Menu.buildFromTemplate([
    ...entries,
    ...(entries.length ? [{ type: 'separator' }] : []),
    { label: '设置', click: openSettings },
    { type: 'separator' },
    { label: '退出', click: () => app.quit() },
  ]));
}

function createTray() {
  let img;
  if (process.platform === 'win32') {
    // Windows 模板图标机制不存在，用彩色图标（深浅任务栏都可见）
    img = nativeImage.createFromPath(path.join(__dirname, 'icon-win.png'));
    if (!img || img.isEmpty()) img = nativeImage.createFromPath(path.join(__dirname, 'icon.png'));
  } else {
    // 18pt 逻辑尺寸；@2x 以 scaleFactor 挂载，Retina 清晰
    try {
      const buf = fs.readFileSync(path.join(__dirname, 'icon@2x.png'));
      img = nativeImage.createFromBuffer(buf, { scaleFactor: 2.0 });
    } catch {}
    if (!img || img.isEmpty()) {
      img = nativeImage.createFromPath(path.join(__dirname, 'icon.png'));
    }
    img.setTemplateImage(true);
  }
  tray = new Tray(img);
  tray.setToolTip('MiMo 仪表盘');
  if (process.platform === 'win32') {
    // Windows 惯例：左键点托盘图标 = 显示/隐藏悬浮窗（右键出菜单）
    tray.on('click', toggleFloat);
  }
  updateTray();
}

function hookRenderer(win, tag) {
  win.webContents.on('did-fail-load', (_e, code, desc) => {
    console.error(`[${tag}] did-fail-load ${code} ${desc}`);
  });
  win.webContents.on('render-process-gone', (_e, details) => {
    console.error(`[${tag}] render gone: ${JSON.stringify(details)}`);
  });
}

// ---------- 悬浮窗（尺寸完全由内容决定，只记忆位置） ----------
function floatPos() {
  if (boundsOnScreen(config.float.bounds)) {
    const b = config.float.bounds;
    return { x: b.x, y: b.y };
  }
  const a = screen.getPrimaryDisplay().workArea;
  return { x: a.x + a.width - 270, y: a.y + 16 };
}

function boundsOnScreen(b) {
  if (!b || ![b.x, b.y, b.width, b.height].every(Number.isFinite)) return false;
  const cx = b.x + b.width / 2;
  const cy = b.y + b.height / 2;
  return screen.getAllDisplays().some((d) => {
    const a = d.workArea;
    return cx >= a.x && cx <= a.x + a.width && cy >= a.y && cy <= a.y + a.height;
  });
}

function toggleFloat() {
  if (floatWin && !floatWin.isDestroyed()) {
    floatWin.close();
    floatWin = null;
    updateTray();
    return;
  }
  const opts = {
    minWidth: 90,   // 尽量贴内容，避免多余留白
    minHeight: 56,
    width: 120,
    height: 70,
    frame: false,
    transparent: true,
    resizable: false, // 尺寸跟数据走，不许手拉
    alwaysOnTop: true,
    skipTaskbar: true,
    hasShadow: false,
    webPreferences: { preload: path.join(__dirname, 'preload.js') },
  };
  const pos = floatPos();
  opts.x = pos.x;
  opts.y = pos.y;
  floatWin = new BrowserWindow(opts);
  floatWin.loadFile('float.html');
  hookRenderer(floatWin, 'float');
  let moveTimer = null;
  const persistPos = () => {
    clearTimeout(moveTimer);
    moveTimer = setTimeout(() => {
      if (floatWin && !floatWin.isDestroyed() && boundsOnScreen(floatWin.getBounds())) {
        config.float.bounds = floatWin.getBounds(); // 只用 x,y；宽高仅作在屏判断
        saveConfig();
      }
    }, 400);
  };
  floatWin.on('moved', persistPos); // 只记位置，尺寸跟数据
  floatWin.once('ready-to-show', () => {
    floatWin.webContents.send('state', publicState());
  });
  floatWin.on('closed', () => {
    floatWin = null;
    // 用户点 × 关窗才持久化为「下次不弹」；退出应用不算
    if (!appQuitting && config.float.show) {
      config.float.show = false;
      saveConfig();
    }
    if (!appQuitting) {
      updateTray();
      broadcast();
    }
  });
}

// ---------- 设置窗 ----------
function openSettings() {
  if (settingsWin && !settingsWin.isDestroyed()) {
    settingsWin.focus();
    return;
  }
  settingsWin = new BrowserWindow({
    width: 460,
    height: 620,
    title: 'MiMo 仪表盘设置',
    webPreferences: { preload: path.join(__dirname, 'preload.js') },
  });
  settingsWin.loadFile('settings.html');
  hookRenderer(settingsWin, 'settings');
  settingsWin.once('ready-to-show', () => {
    settingsWin.webContents.send('state', publicState());
  });
  settingsWin.on('closed', () => {
    settingsWin = null;
  });
}

// ---------- IPC ----------
ipcMain.handle('get-state', () => publicState());
ipcMain.handle('refresh-now', async () => {
  await refresh();
  return { error: errorMsg };
});
ipcMain.on('save-config', (_e, next) => {
  config = {
    ...config,
    ...next,
    float: { ...config.float, ...(next.float || {}) },
    tray: { ...config.tray, ...(next.tray || {}) },
  };
  saveConfig();
  try {
    app.setLoginItemSettings({ openAtLogin: !!config.launchAtLogin });
  } catch (e) {
    console.error('[login-item] failed', e);
  }
  startTimer();
  // 悬浮窗开/关与配置对齐（设置页胶囊驱动）
  const wantFloat = !!config.float.show;
  if (wantFloat && (!floatWin || floatWin.isDestroyed())) toggleFloat();
  else if (!wantFloat && floatWin && !floatWin.isDestroyed()) floatWin.close();
  broadcast(); // 立即生效：只刷新 UI，不重新拉数
});
ipcMain.on('set-float-size', (_e, w, h) => {
  if (!floatWin || floatWin.isDestroyed()) return;
  if (!Number.isFinite(w) || !Number.isFinite(h) || w <= 0 || h <= 0) return;
  const width = Math.max(90, Math.min(2400, Math.ceil(w)));
  const height = Math.max(56, Math.min(2000, Math.ceil(h)));
  const cur = floatWin.getBounds();
  if (Math.abs(cur.width - width) <= 1 && Math.abs(cur.height - height) <= 1) return;
  floatWin.setBounds({ x: cur.x, y: cur.y, width, height });
});
ipcMain.on('copy-text', (_e, t) => {
  if (typeof t === 'string' && t.length < 500) clipboard.writeText(t);
});

app.whenReady().then(() => {
  console.log('[boot] ready');
  app.dock && app.dock.hide();
  try {
    app.setLoginItemSettings({ openAtLogin: !!config.launchAtLogin });
  } catch {}
  createTray();
  console.log('[boot] tray ok');
  refresh();
  startTimer();
  if (config.float.show) toggleFloat(); // 按持久化选项决定是否显示
  console.log('[boot] float ok');
}).catch((e) => console.error('[boot] failed', e));

app.on('before-quit', () => {
  appQuitting = true;
});
app.on('window-all-closed', () => {}); // 托盘常驻，不退出
