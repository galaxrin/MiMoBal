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
  app.setAppUserModelId('com.galaxrin.mimobal');
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
  const arr = [];
  const known = new Set();
  if (Array.isArray(list)) {
    for (const f of list) {
      if (f && typeof f.id === 'string' && !known.has(f.id) && FIELDS.some((m) => m.id === f.id)) {
        known.add(f.id);
        arr.push({ id: f.id, on: !!f.on });
      }
    }
  }
  for (const m of FIELDS) {
    if (!known.has(m.id)) {
      known.add(m.id);
      arr.push({ id: m.id, on: false });
    }
  }
  return arr;
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
  // 手改 config.json 可能写入非法值（NaN/字符串）：这里收敛，避免 setInterval(NaN) 轮询风暴等
  const rs = Number(raw.refreshSec);
  const refreshSec = Number.isFinite(rs)
    ? Math.min(86400, Math.max(10, Math.round(rs)))
    : DEFAULT_CONFIG.refreshSec;
  const op = Number(raw.float && raw.float.opacity);
  const opacity = Number.isFinite(op) ? Math.min(1, Math.max(0.3, op)) : DEFAULT_CONFIG.float.opacity;
  // 迁移：旧版单一 fields 列表 → 悬浮窗/托盘两份
  const legacyFields = Array.isArray(raw.fields) ? raw.fields : null;
  return {
    ...DEFAULT_CONFIG,
    ...raw,
    fields: undefined, // 旧键废弃，不再持久化
    refreshSec,
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
      opacity,
      fields: mergeFieldList(
        (raw.float && raw.float.fields) || legacyFields || DEFAULT_CONFIG.float.fields
      ),
      heroIds: (raw.float && Array.isArray(raw.float.heroIds) && raw.float.heroIds.length)
        ? raw.float.heroIds.slice(0, 2).map((x) => (typeof x === 'string' && x) || null)
        : DEFAULT_CONFIG.float.heroIds,
      scale: undefined, // 弃用：改窗口拉伸，缩放记忆走 bounds
      accent: undefined, // 弃用：主题色选项移除，固定默认色
    },
  };
}

// 改名迁移：userData 目录随 productName 变化（mimo-dash → MiMoBal），
// 首次启动把旧配置搬过来，避免开关/字段设置丢失
(function migrateOldUserData() {
  try {
    const oldDir = path.join(app.getPath('appData'), 'mimo-dash');
    const newDir = app.getPath('userData');
    if (path.resolve(oldDir) === path.resolve(newDir)) return;
    const src = path.join(oldDir, 'config.json');
    const dst = path.join(newDir, 'config.json');
    if (fs.existsSync(src) && !fs.existsSync(dst)) {
      fs.mkdirSync(newDir, { recursive: true });
      fs.copyFileSync(src, dst);
      console.log('[migrate] 旧配置已迁移到', dst);
    }
  } catch (e) {
    console.warn('[migrate] 旧配置迁移失败:', e.message);
  }
})();

let config = loadConfig();

// 双端托盘常驻：防止重复启动多开（第二实例聚焦设置）
const isPrimary = app.requestSingleInstanceLock();
if (!isPrimary) {
  app.quit();
} else {
  app.on('second-instance', () => {
    if (settingsWin && !settingsWin.isDestroyed()) settingsWin.focus();
    else openSettings();
  });
}

function saveConfig() {
  try {
    fs.writeFileSync(configPath(), JSON.stringify(config, null, 2));
  } catch (e) {
    console.error('[config] save failed:', e.message);
  }
}
let appQuitting = false;

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
          title: `MiMoBal · ${label}触发阈值`,
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

// 凭证过期/被拒后暂停登录重试（手动「重试」可立即放行），避免每 30 秒打爆登录接口加剧风控
// 只对「服务端拒绝/已失效」退避；「未找到 passToken」应允许用户登录后立刻自动重试
let authFailUntil = 0;
const AUTH_BACKOFF_MS = 5 * 60 * 1000;
function authLooksFailed(msg) {
  return /凭证已失效|登录验证失败|凭证重试次数已用尽|EXPIRED|70016/.test(msg || '');
}

// 同一时刻只跑一轮刷新：慢网络下定时器/手动重试都复用进行中的请求
let refreshPromise = null;
function refresh({ force = false } = {}) {
  if (refreshPromise) return refreshPromise;
  if (!force && Date.now() < authFailUntil && authLooksFailed(errorMsg)) {
    broadcast();
    return Promise.resolve();
  }
  refreshPromise = (async () => {
    try {
      data = await snapshot();
      errorMsg = null;
      authFailUntil = 0;
      checkThresholdAlerts();
    } catch (e) {
      errorMsg = String(e && e.message ? e.message : e);
      if (authLooksFailed(errorMsg)) authFailUntil = Date.now() + AUTH_BACKOFF_MS;
    }
    broadcast();
  })().finally(() => {
    refreshPromise = null;
  });
  return refreshPromise;
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
    // 菜单栏空间有限：过长摘要截断，避免挤掉时钟
    const max = 72;
    tray.setTitle(summary ? ` ${summary.slice(0, max)}` : '');
  }
  // 悬停 tooltip：各平台都带上摘要，Windows 上这是主要的“扫一眼”入口
  const tip = summary ? `MiMoBal · ${summary}` : 'MiMoBal';
  tray.setToolTip(tip.length > 120 ? `${tip.slice(0, 119)}…` : tip);
  // 下拉：设置 + 退出；Windows 额外把已勾选字段列成菜单项（替代托盘文字）
  const entries = process.platform !== 'darwin' && summary
    ? orderedEntries('tray').map((e) => ({ label: `${e.label}  ${e.text}`, enabled: false }))
    : [];
  const floatOn = !!(floatWin && !floatWin.isDestroyed());
  tray.setContextMenu(Menu.buildFromTemplate([
    ...entries,
    ...(entries.length ? [{ type: 'separator' }] : []),
    {
      label: floatOn ? '隐藏悬浮窗' : '显示悬浮窗',
      click: () => {
        config.float.show = !floatOn;
        saveConfig();
        if (config.float.show) toggleFloat();
        else if (floatWin && !floatWin.isDestroyed()) floatWin.close();
        broadcast();
      },
    },
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
    // 逻辑尺寸；@2x 以 scaleFactor 挂载，Retina 清晰
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
  tray.setToolTip('MiMoBal');
  if (process.platform === 'win32') {
    // Windows：左键点托盘 = 打开设置（悬浮窗显隐在托盘菜单里切换）
    tray.on('click', openSettings);
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
  win.webContents.on('console-message', (details) => {
    if (details && details.level === 'error') {
      console.error(`[${tag}] ${details.message} (${details.sourceId}:${details.lineNumber})`);
    }
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
    title: 'MiMoBal',
    minWidth: 90,   // 尽量贴内容，避免多余留白
    minHeight: 56,
    width: 120,
    height: 70,
    frame: false,
    // Win11 透明窗角部残影：保持不透明 + DWM 原生圆角。
    // macOS 真透明，透明度滑条才有「透出桌面」效果。
    transparent: process.platform !== 'win32',
    backgroundColor: '#000000',
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
  // 打开时写入记忆（与关窗对称）；设置胶囊打开时 show 已为 true，此处为兜底
  if (!config.float.show) {
    config.float.show = true;
    saveConfig();
    if (settingsWin && !settingsWin.isDestroyed()) broadcast();
  }
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
    // 程序化关闭（设置胶囊关掉悬浮窗等）时同步记忆；退出应用不算
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
    title: 'MiMoBal 设置',
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
  authFailUntil = 0; // 用户手动重试 = 明确放行登录（例如刚在 MiMo Desktop 重新登录）
  await refresh({ force: true });
  return { error: errorMsg };
});
ipcMain.on('save-config', (_e, next) => {
  config = {
    ...config,
    ...next,
    // bounds 只由主进程在窗口移动时写入：渲染端（设置窗）持有的可能是过期快照，
    // 直接采纳会把刚拖到位的悬浮窗重置回旧位置
    float: { ...config.float, ...(next.float || {}), bounds: config.float.bounds },
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
  // 内容变宽时若顶点已贴近屏幕右缘，向左挪，避免增长后滑出工作区
  const wa = screen.getDisplayMatching(cur).workArea;
  let x = cur.x;
  if (x + width > wa.x + wa.width) x = Math.max(wa.x, wa.x + wa.width - width);
  if (x < wa.x) x = wa.x;
  // Windows 上创建时 resizable:false 的窗口可能忽略程序化尺寸变更，
  // 先临时放开、改完再收回（视觉无感，用户侧仍不可拖拽）
  floatWin.setResizable(true);
  floatWin.setBounds({ x, y: cur.y, width, height });
  floatWin.setResizable(false);
});
ipcMain.on('copy-text', (_e, t) => {
  if (typeof t === 'string' && t.length < 500) clipboard.writeText(t);
});

app.whenReady().then(() => {
  if (!isPrimary) return;
  console.log('[boot] ready', process.platform);
  // macOS：菜单栏常驻，隐藏 Dock（打包侧 LSUIElement 双保险）
  app.dock && app.dock.hide();
  try {
    app.setLoginItemSettings({ openAtLogin: !!config.launchAtLogin });
  } catch (e) {
    console.error('[login-item] failed', e);
  }
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
// 托盘常驻：关窗不退出（macOS/Windows/Linux 一致）
app.on('window-all-closed', () => {});
