// MiMo 平台数据层：复用 MiMo Desktop 的小米账号 passToken 换平台会话
// 链路（已用 Python 版验证）：serviceLogin(sid=api-platform) -> /sts -> Cookie -> /api/v1/*
//
// 凭证读取顺序：
//   1) xiaomi-account 分区 Cookies SQLite —— macOS 可直接读；Windows 上该库通常被
//      MiMo Desktop 的 Chromium Network Service 独占锁定，读不到时进入 2)
//   2) 分区 HTTP 缓存（Cache/Cache_Data/*）里缓存的 serviceLogin 响应 JSON ——
//      其中回显了当前 passToken；缓存块文件也可能被短时锁定，逐文件跳过 + 整轮重试
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const { DatabaseSync } = require('node:sqlite');

// MiMo Desktop 的 userData 目录（区别于本应用自己的 userData）
function desktopUserData() {
  if (process.platform === 'win32') {
    return path.join(
      process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'),
      'Xiaomi MiMo'
    );
  }
  if (process.platform === 'darwin') {
    return path.join(os.homedir(), 'Library', 'Application Support', 'Xiaomi MiMo');
  }
  return path.join(os.homedir(), '.config', 'Xiaomi MiMo');
}

// Chromium 布局差异：新版在 Network/Cookies，旧版在分区根
function cookieDbCandidates() {
  const partition = path.join(desktopUserData(), 'Partitions', 'xiaomi-account');
  return [
    path.join(partition, 'Network', 'Cookies'),
    path.join(partition, 'Cookies'),
  ];
}

const API = 'https://platform.xiaomimimo.com/api/v1';
const markerPassToken = Buffer.from('passToken');
const UA = process.platform === 'win32'
  ? 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36'
  : process.platform === 'darwin'
    ? 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)'
    : 'Mozilla/5.0 (X11; Linux x86_64)';

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
// 服务端拒绝过的凭证：登录时逐枚换候选，避免死磕同一枚坏 token
const rejectedTokens = new Set();

function credsFromCookieDb(dbPath) {
  if (!fs.existsSync(dbPath)) throw new Error(`cookie db missing: ${dbPath}`);
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mimocookie_'));
  try {
    for (const suffix of ['', '-wal', '-shm']) {
      const src = dbPath + suffix;
      if (fs.existsSync(src)) fs.copyFileSync(src, path.join(tmp, path.basename(dbPath) + suffix));
    }
    const db = new DatabaseSync(path.join(tmp, path.basename(dbPath)));
    const rows = db.prepare('select name, value from cookies').all();
    db.close();
    const map = Object.fromEntries(rows.map((r) => [r.name, r.value]));
    if (!map.passToken || !map.userId) {
      throw new Error('cookie db 里没有 passToken/userId');
    }
    return map;
  } finally {
    // 库损坏/被锁时也要清掉临时副本，避免 %TEMP% 堆积
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

// 从含 passToken 的字节流里解析出完整 JSON 对象（大括号配对，跳过字符串内部）
function extractLoginPayloads(buf) {
  const marker = Buffer.from('passToken');
  const payloads = [];
  let from = 0;
  for (;;) {
    const hit = buf.indexOf(marker, from);
    if (hit < 0) break;
    from = hit + marker.length;
    // 从命中位置向前找包围它的 '{'（由内向外逐层尝试）
    const openers = [];
    let depth = 0;
    for (let j = hit - 1; j >= 0 && openers.length < 8; j--) {
      const c = buf[j];
      if (c === 0x7d) depth++;
      else if (c === 0x7b) {
        if (depth === 0) openers.push(j);
        else depth--;
      }
    }
    for (const start of openers) {
      const end = matchBrace(buf, start);
      if (end < 0) continue;
      try {
        const obj = JSON.parse(buf.toString('utf8', start, end + 1));
        if (obj && typeof obj.passToken === 'string' && obj.passToken) {
          payloads.push({ obj, start });
          break;
        }
      } catch { /* 该层不是完整 JSON，继续向外层试 */ }
    }
  }
  return payloads;
}

function matchBrace(buf, start) {
  let depth = 0;
  let inStr = false;
  let esc = false;
  for (let j = start; j < buf.length; j++) {
    const c = buf[j];
    if (inStr) {
      if (esc) esc = false;
      else if (c === 0x5c) esc = true;
      else if (c === 0x22) inStr = false;
      continue;
    }
    if (c === 0x22) inStr = true;
    else if (c === 0x7b) depth++;
    else if (c === 0x7d) {
      depth--;
      if (depth === 0) return j;
    }
  }
  return -1;
}

function userIdFallback() {
  try {
    const p = path.join(desktopUserData(), 'xiaomi-last-confirmed.json');
    const j = JSON.parse(fs.readFileSync(p, 'utf8'));
    if (j && j.userId) return String(j.userId);
  } catch { /* 不存在或损坏则忽略 */ }
  return null;
}

// 扫描分区 HTTP 缓存，返回全部可用凭证候选（按文件 mtime 降序，按 passToken 去重）。
// 缓存块可能被 Network Service 短时占用：逐文件跳过 + 整轮异步重试（不阻塞主进程）。
async function collectCacheCandidates() {
  const dir = path.join(desktopUserData(), 'Partitions', 'xiaomi-account', 'Cache', 'Cache_Data');
  let names;
  try {
    names = fs.readdirSync(dir).sort();
  } catch {
    return { cands: [], hadLocks: false, dirMissing: true };
  }
  let hadLocks = false;
  for (let round = 0; round < 4; round++) {
    const byToken = new Map();
    let anyUnreadable = false;
    for (const name of names) {
      let buf;
      try {
        buf = fs.readFileSync(path.join(dir, name));
      } catch {
        anyUnreadable = true; // 被占用，跳过
        continue;
      }
      if (!buf.includes(markerPassToken)) continue;
      const hits = extractLoginPayloads(buf);
      if (!hits.length) continue;
      let mtime = 0;
      try { mtime = fs.statSync(path.join(dir, name)).mtimeMs; } catch {}
      for (const h of hits) {
        const o = h.obj;
        if (!o.userId && !userIdFallback()) continue;
        const cand = {
          passToken: o.passToken,
          userId: o.userId ? String(o.userId) : userIdFallback(),
          cUserId: o.cUserId ? String(o.cUserId) : undefined,
          mtime,
        };
        const prev = byToken.get(cand.passToken);
        if (!prev || mtime > prev.mtime) byToken.set(cand.passToken, cand);
      }
    }
    hadLocks = hadLocks || anyUnreadable;
    const cands = [...byToken.values()].sort((a, b) => b.mtime - a.mtime);
    if (cands.length) return { cands, hadLocks, dirMissing: false };
    if (!anyUnreadable) return { cands: [], hadLocks, dirMissing: false }; // 全读到了还命中不了 = 缓存里确实没有
    if (round < 3) await delay(400);
  }
  return { cands: [], hadLocks, dirMissing: false };
}

async function readPassToken() {
  let lastErr = null;
  for (const db of cookieDbCandidates()) {
    try {
      const creds = credsFromCookieDb(db);
      if (!rejectedTokens.has(creds.passToken)) return creds;
    } catch (e) {
      lastErr = e;
    }
  }
  const { cands, hadLocks, dirMissing } = await collectCacheCandidates();
  const usable = cands.filter((c) => !rejectedTokens.has(c.passToken));
  if (usable.length) return usable[0];
  if (cands.length) {
    // 候选都在，但全被服务端拒过/已过期 —— 该换登录态了
    throw new Error('登录凭证已失效，请在 MiMo Desktop 重新登录');
  }
  const winHint = process.platform === 'win32' && hadLocks
    ? '；若已登录，可能是凭证文件暂时被占用，刷新时会自动重试'
    : '';
  throw new Error(
    '未找到小米账号 passToken，请先在 MiMo Desktop 登录' +
    winHint +
    (dirMissing ? '（未找到 MiMo Desktop 的缓存目录）' : '') +
    (lastErr ? `（${lastErr.message}）` : '')
  );
}

class Session {
  constructor() {
    this.cookies = new Map();
    this.loginPromise = null;
  }

  ensureLogin() {
    if (!this.loginPromise) {
      this.loginPromise = this.login().catch((e) => {
        this.loginPromise = null;
        throw e;
      });
    }
    return this.loginPromise;
  }

  cookieHeader() {
    return [...this.cookies.entries()].map(([k, v]) => `${k}=${v}`).join('; ');
  }

  async request(url, { method = 'GET', headers = {}, redirect = 'manual' } = {}) {
    const resp = await fetch(url, {
      method,
      headers: { 'User-Agent': UA, Cookie: this.cookieHeader(), ...headers },
      redirect,
      // 单请求超时：挂起的连接会卡住整轮刷新（主进程刷新依赖本 promise 结束）
      signal: AbortSignal.timeout(15000),
    });
    const setCookies = typeof resp.headers.getSetCookie === 'function'
      ? resp.headers.getSetCookie()
      : [];
    for (const c of setCookies) {
      const pair = c.split(';')[0];
      const eq = pair.indexOf('=');
      if (eq > 0) {
        const name = pair.slice(0, eq).trim();
        const value = pair.slice(eq + 1).trim();
        // 空值 Set-Cookie（如 userId=）只用于清 Cookie，忽略以免覆盖账号态
        if (value !== '') this.cookies.set(name, value);
      }
    }
    return resp;
  }

  // 凭证可能过期：服务端拒绝一枚就拉黑换下一枚，最多试 3 枚候选
  async login() {
    let credErr = null;
    for (let attempt = 0; attempt < 3; attempt++) {
      const acc = await readPassToken();
      this.cookies.clear();
      this.cookies.set('passToken', acc.passToken);
      this.cookies.set('userId', acc.userId);
      if (acc.cUserId) this.cookies.set('cUserId', acc.cUserId);
      try {
        await this.loginWithCreds();
        return;
      } catch (e) {
        if (e && e.credRejected) {
          rejectedTokens.add(acc.passToken);
          credErr = e;
          continue;
        }
        // 后续候选已全部拉黑时 readPassToken 会抛笼统错误，保留更具体的首次登录失败原因
        throw credErr || e;
      }
    }
    throw credErr || new Error('凭证重试次数已用尽');
  }

  async loginWithCreds() {
    // 回调必须用平台 401 响应里的 loginUrl（自带 sign），自拼会被拒
    const probe = await this.request(`${API}/balance`);
    const info = await probe.json().catch(() => ({}));
    const loginUrl = info.loginUrl;
    if (!loginUrl) throw new Error(`拿不到 loginUrl: ${JSON.stringify(info)}`);
    const resp = await this.request(loginUrl + (loginUrl.includes('_json') ? '' : '&_json=true'));
    const body = await resp.text();
    const payload = JSON.parse(body.split('&&&START&&&').pop());
    if (payload.code !== 0 || !payload.location) {
      // 70016/「登录验证失败」多为缓存里 passToken 已过期（Windows 上 Cookie 库被占用时
      // 只能读到缓存旧凭证），提示重登比笼统的「被拒绝」更可操作
      const desc = String(payload.desc || payload.description || '');
      const expired = payload.code === 70016 || desc.includes('验证') || desc.includes('过期');
      const err = new Error(
        expired
          ? '登录凭证已失效，请在 MiMo Desktop 重新登录'
          : `小米登录失败: ${desc || payload.code}`
      );
      err.credRejected = true; // 服务端明确拒绝这枚凭证 → 拉黑，外层换下一枚
      throw err;
    }
    // 跟随 location：account -> /sts（下发平台 Cookie）
    let loc = payload.location;
    for (let i = 0; i < 5 && loc; i++) {
      const r = await this.request(loc);
      if (r.status >= 300 && r.status < 400) {
        loc = r.headers.get('location');
      } else {
        await r.text();
        loc = null;
      }
    }
  }

  async get(apiPath) {
    const url = `${API}/${apiPath}`;
    for (let attempt = 0; attempt < 2; attempt++) {
      await this.ensureLogin();
      const resp = await this.request(url);
      const data = await resp.json();
      const unauthorized = resp.status === 401 || resp.status === 403 || data.code === 401;
      if (unauthorized && attempt === 0) {
        this.loginPromise = null;
        continue;
      }
      if (data.code !== 0) throw new Error(`API 错误: ${JSON.stringify(data)}`);
      return data.data ?? data;
    }
    throw new Error('unreachable');
  }
}

// ---------- 会话复用：跨刷新共用登录态，401 时重建 ----------
let platformSession = null;
let subCookies = null; // mimopc SSO cookie（模块级，失败时清空重登）

function mergeCookies(cookies, resp) {
  for (const c of resp.headers.getSetCookie?.() || []) {
    const pair = c.split(';')[0];
    const eq = pair.indexOf('=');
    if (eq > 0) {
      const value = pair.slice(eq + 1).trim();
      if (value !== '') cookies.set(pair.slice(0, eq).trim(), value);
    }
  }
}

function subReq(cookies, url, { sendAccount = true, redirect = 'manual' } = {}) {
  const headers = { 'User-Agent': UA };
  if (sendAccount) {
    headers.Cookie = [...cookies].map(([k, v]) => `${k}=${v}`).join('; ');
  }
  return fetch(url, { headers, redirect, signal: AbortSignal.timeout(15000) }).then((resp) => {
    mergeCookies(cookies, resp);
    return resp;
  });
}

// 桌面端订阅（D7）：mimo-server 独立 SSO（sid=mimopc）
// 链路：/api/user/xiaomi/me 302 签名 loginUrl -> serviceLogin(passToken) -> /api/sts 下发 cookie
async function mimopcLogin() {
  const cookies = new Map();
  const acc = await readPassToken();
  cookies.set('passToken', acc.passToken);
  cookies.set('userId', acc.userId);
  if (acc.cUserId) cookies.set('cUserId', acc.cUserId);

  const base = 'https://mimo-server-cn.xiaomimimo.com/api';
  const probe = await subReq(cookies, `${base}/user/xiaomi/me`, { sendAccount: false });
  // 已登录：接口直接 200 + code=0，不再走 302 loginUrl
  if (probe.ok) {
    const j = await probe.json().catch(() => null);
    if (j && j.code === 0) return cookies;
  }
  const loginUrl = probe.headers.get('location');
  if (!loginUrl) return null;

  const r1 = await subReq(cookies, loginUrl + '&_json=true');
  const payload = JSON.parse((await r1.text()).split('&&&START&&&').pop());
  if (payload.code !== 0 || !payload.location) return null;

  let loc = payload.location;
  for (let i = 0; i < 5 && loc; i++) {
    const r = await subReq(cookies, loc);
    if (r.status >= 300 && r.status < 400) loc = r.headers.get('location');
    else {
      await r.text();
      loc = null;
    }
  }
  return cookies;
}

async function fetchSubscription() {
  try {
    if (!subCookies) subCookies = await mimopcLogin();
    if (!subCookies) return null;
    const base = 'https://mimo-server-cn.xiaomimimo.com/api';
    const authed = async (path) => {
      let r = await subReq(subCookies, `${base}/${path}`);
      if (r.status === 401) {
        subCookies = await mimopcLogin();
        if (!subCookies) return null;
        r = await subReq(subCookies, `${base}/${path}`);
      }
      if (!r.ok) return null;
      const j = await r.json();
      return j.code === 0 ? j.data : null;
    };
    const [sub, usage] = await Promise.all([
      authed('user/xiaomi/subscription/self'),
      authed('user/usage'),
    ]);
    if (!sub && !usage) return null;
    return { plan: sub && sub.current, usage };
  } catch {
    subCookies = null;
    return null;
  }
}

async function snapshot() {
  if (!platformSession) platformSession = new Session();
  const [balance, usage, detail, subscription] = await Promise.all([
    platformSession.get('balance'),
    platformSession.get('tokenPlan/usage'),
    platformSession.get('tokenPlan/detail'),
    fetchSubscription(),
  ]);
  return { balance, usage, detail, subscription, ts: Date.now() };
}

module.exports = { snapshot };
