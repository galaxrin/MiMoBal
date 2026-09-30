// MiMo 平台数据层：复用 MiMo Desktop 的小米账号 passToken 换平台会话
// 链路（已用 Python 版验证）：serviceLogin(sid=api-platform) -> /sts -> Cookie -> /api/v1/*
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const { DatabaseSync } = require('node:sqlite');

const COOKIE_DB = path.join(
  os.homedir(),
  'Library/Application Support/Xiaomi MiMo/Partitions/xiaomi-account/Cookies'
);
const API = 'https://platform.xiaomimimo.com/api/v1';
const UA = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)';

function readPassToken() {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mimocookie_'));
  for (const suffix of ['', '-wal', '-shm']) {
    const src = COOKIE_DB + suffix;
    if (fs.existsSync(src)) fs.copyFileSync(src, path.join(tmp, path.basename(COOKIE_DB) + suffix));
  }
  const db = new DatabaseSync(path.join(tmp, path.basename(COOKIE_DB)));
  const rows = db.prepare('select name, value from cookies').all();
  db.close();
  fs.rmSync(tmp, { recursive: true, force: true });
  const map = Object.fromEntries(rows.map((r) => [r.name, r.value]));
  if (!map.passToken || !map.userId) {
    throw new Error('未找到小米账号 passToken，请先在 MiMo Desktop 登录');
  }
  return map;
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

  async login() {
    const acc = readPassToken();
    this.cookies.set('passToken', acc.passToken);
    this.cookies.set('userId', acc.userId);
    if (acc.cUserId) this.cookies.set('cUserId', acc.cUserId);

    // 回调必须用平台 401 响应里的 loginUrl（自带 sign），自拼会被拒
    const probe = await this.request(`${API}/balance`);
    const info = await probe.json().catch(() => ({}));
    const loginUrl = info.loginUrl;
    if (!loginUrl) throw new Error(`拿不到 loginUrl: ${JSON.stringify(info)}`);
    const resp = await this.request(loginUrl + (loginUrl.includes('_json') ? '' : '&_json=true'));
    const body = await resp.text();
    const payload = JSON.parse(body.split('&&&START&&&').pop());
    if (payload.code !== 0 || !payload.location) {
      throw new Error(`小米登录失败: ${payload.desc || payload.description || payload.code}`);
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
  return fetch(url, { headers, redirect }).then((resp) => {
    mergeCookies(cookies, resp);
    return resp;
  });
}

// 桌面端订阅（D7）：mimo-server 独立 SSO（sid=mimopc）
// 链路：/api/user/xiaomi/me 302 签名 loginUrl -> serviceLogin(passToken) -> /api/sts 下发 cookie
async function mimopcLogin() {
  const cookies = new Map();
  const acc = readPassToken();
  cookies.set('passToken', acc.passToken);
  cookies.set('userId', acc.userId);
  if (acc.cUserId) cookies.set('cUserId', acc.cUserId);

  const base = 'https://mimo-server-cn.xiaomimimo.com/api';
  const probe = await subReq(cookies, `${base}/user/xiaomi/me`, { sendAccount: false });
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
