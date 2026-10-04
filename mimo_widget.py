#!/usr/bin/env python3
"""MiMo 余额 + Token Plan 桌面悬浮窗。

注意：本文件是 macOS 原型存档，已被 Electron 版（main.js/data.js）取代且未适配 Windows；
Windows 上凭证读取请参考 data.js 的“Cookie 库 → HTTP 缓存”双路回退实现。

数据源：platform.xiaomimimo.com /api/v1/{balance,tokenPlan/*}
鉴权：复用 MiMo Desktop xiaomi-account 分区里的 passToken，走小米 serviceLogin 换平台会话。

用法：
  python3 mimo_widget.py          # 打开悬浮窗
  python3 mimo_widget.py --fetch  # 拉一次数据打印 JSON（自检）
"""
import http.cookiejar
import json
import os
import shutil
import sqlite3
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

API = "https://platform.xiaomimimo.com/api/v1"
COOKIE_DB = os.path.expanduser(
    "~/Library/Application Support/Xiaomi MiMo/Partitions/xiaomi-account/Cookies"
)
REFRESH_S = 60
UA = {"User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"}


def read_pass_token():
    tmp = tempfile.mkdtemp(prefix="mimocookie_")
    for suffix in ("", "-wal", "-shm"):
        src = COOKIE_DB + suffix
        if os.path.exists(src):
            shutil.copy(src, os.path.join(tmp, os.path.basename(src)))
    db = os.path.join(tmp, os.path.basename(COOKIE_DB))
    con = sqlite3.connect(db)
    rows = dict(con.execute("select name, value from cookies").fetchall())
    con.close()
    shutil.rmtree(tmp, ignore_errors=True)
    need = ("passToken", "userId")
    if any(not rows.get(k) for k in need):
        raise RuntimeError("未找到小米账号 passToken，请先在 MiMo Desktop 登录")
    return rows["passToken"], rows.get("userId", ""), rows.get("cUserId", "")


class Session:
    """平台会话：懒初始化，401 自动重登一次。"""

    def __init__(self):
        self.jar = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(self.jar)
        )
        self._logged_in = False

    def login(self, login_url):
        pass_token, user_id, c_user_id = read_pass_token()
        req = urllib.request.Request(login_url + "&_json=true", headers={
            **UA,
            "Cookie": f"passToken={pass_token}; userId={user_id}; cUserId={c_user_id}",
        })
        body = self.opener.open(req, timeout=15).read().decode()
        payload = json.loads(body.split("&&&START&&&")[-1])
        if payload.get("code") != 0 or not payload.get("location"):
            raise RuntimeError(f"小米登录失败: {payload.get('desc') or payload.get('description')}")
        # 跟随 location：account → /sts（下发平台 Cookie）→ followup API
        self.opener.open(urllib.request.Request(payload["location"], headers=UA), timeout=15)
        self._logged_in = True

    def get(self, path):
        url = f"{API}/{path}"
        if not self._logged_in:
            self._bootstrap(url)
        try:
            return self._json(self.opener.open(
                urllib.request.Request(url, headers=UA), timeout=15))
        except urllib.error.HTTPError as e:
            if e.code in (401, 403):  # 会话过期，重登一次
                self._logged_in = False
                self._bootstrap(url)
                return self._json(self.opener.open(
                    urllib.request.Request(url, headers=UA), timeout=15))
            raise

    def _bootstrap(self, api_url):
        try:
            resp = urllib.request.urlopen(
                urllib.request.Request(api_url, headers=UA), timeout=15)
            info = json.loads(resp.read().decode())
        except urllib.error.HTTPError as e:
            info = json.loads(e.read().decode() or "{}")
        login_url = info.get("loginUrl")
        if not login_url:
            raise RuntimeError(f"拿不到 loginUrl: {info}")
        self.login(login_url)

    @staticmethod
    def _json(resp):
        data = json.loads(resp.read().decode())
        if data.get("code") not in (0, None):
            raise RuntimeError(f"API 错误: {data}")
        return data.get("data", data)


def snapshot(sess):
    balance = sess.get("balance")
    usage = sess.get("tokenPlan/usage")
    detail = sess.get("tokenPlan/detail")
    return {"balance": balance, "usage": usage, "detail": detail, "ts": time.time()}


def fmt_tokens(n):
    for unit, div in (("B", 1e9), ("M", 1e6), ("K", 1e3)):
        if n >= div:
            return f"{n / div:.2f}{unit}"
    return str(n)


# ---------------- Tkinter 悬浮窗 ----------------

def run_widget():
    import tkinter as tk

    BG, FG, ACCENT, DIM = "#16181d", "#e8eaed", "#4c8dff", "#8b919a"
    BORDER = "#2a2e36"
    state = {"data": None, "error": None, "stamp": 0.0}
    lock = threading.Lock()

    def worker():
        sess = Session()
        while True:
            try:
                data = snapshot(sess)
                with lock:
                    state.update(data=data, error=None, stamp=time.time())
            except Exception as e:  # 显示错误即可，窗口不崩
                with lock:
                    state.update(error=str(e), stamp=time.time())
            time.sleep(REFRESH_S)

    root = tk.Tk()
    root.overrideredirect(True)
    root.attributes("-topmost", True)
    root.configure(bg=BORDER)

    win = tk.Frame(root, bg=BORDER, highlightbackground=BORDER, highlightthickness=1)
    win.pack(padx=1, pady=1)

    header = tk.Frame(win, bg=BG)
    header.pack(fill="x", padx=10, pady=(8, 0))
    tk.Label(header, text="MiMo 仪表盘", bg=BG, fg=FG,
             font=("PingFang SC", 12, "bold")).pack(side="left")
    close_btn = tk.Label(header, text="✕", bg=BG, fg=DIM, cursor="hand2",
                         font=("Helvetica", 11))
    close_btn.pack(side="right")
    close_btn.bind("<Button-1>", lambda e: root.destroy())

    bal_row = tk.Frame(win, bg=BG)
    bal_row.pack(fill="x", padx=10, pady=(6, 0))
    bal_val = tk.Label(bal_row, text="余额 …", bg=BG, fg=FG,
                       font=("PingFang SC", 20, "bold"))
    bal_val.pack(side="left")
    bal_sub = tk.Label(bal_row, text="", bg=BG, fg=DIM, font=("PingFang SC", 10))
    bal_sub.pack(side="right", pady=(8, 0))

    plan_lbl = tk.Label(win, text="Token Plan …", bg=BG, fg=FG,
                        font=("PingFang SC", 11), anchor="w")
    plan_lbl.pack(fill="x", padx=10, pady=(8, 0))

    bar_bg = tk.Frame(win, bg="#3a3f4a", height=8)
    bar_bg.pack(fill="x", padx=10, pady=(4, 0))
    bar_bg.pack_propagate(False)
    bar_fg = tk.Frame(bar_bg, bg=ACCENT, width=0, height=8)
    bar_fg.pack(side="left", fill="y")

    usage_lbl = tk.Label(win, text="", bg=BG, fg=DIM,
                         font=("PingFang SC", 10), anchor="w")
    usage_lbl.pack(fill="x", padx=10, pady=(3, 0))

    status_lbl = tk.Label(win, text="加载中…", bg=BG, fg=DIM,
                          font=("PingFang SC", 9), anchor="w")
    status_lbl.pack(fill="x", padx=10, pady=(4, 8))

    # 拖拽移动
    drag = {"x": 0, "y": 0}
    def on_press(e):
        drag.update(x=e.x_root, y=e.y_root)
    def on_move(e):
        root.geometry(f"+{root.winfo_x() + e.x_root - drag['x']}"
                      f"+{root.winfo_y() + e.y_root - drag['y']}")
        drag.update(x=e.x_root, y=e.y_root)
    for w in (header, bal_row, plan_lbl, usage_lbl, status_lbl):
        w.bind("<Button-1>", on_press)
        w.bind("<B1-Motion>", on_move)
    header.bind("<Button-1>", on_press)

    def render():
        with lock:
            data, error, stamp = state["data"], state["error"], state["stamp"]
        if error:
            status_lbl.config(text=f"⚠ {error}", fg="#e06c75")
        elif data:
            b = data["balance"]
            cur = b.get("currency", "CNY")
            bal_val.config(text=f"¥{b.get('balance', '?')}" if cur == "CNY"
                           else f"{b.get('balance', '?')} {cur}")
            frozen = b.get("frozenBalance", "0.00")
            bal_sub.config(text=f"冻结 {frozen}" if frozen not in ("0.00", "0") else "")

            detail = data["detail"] or {}
            period = detail.get("currentPeriodEnd", "")
            renew = "自动续订" if detail.get("enableAutoRenew") else "未开自动续订"
            plan_lbl.config(
                text=f"{detail.get('planName', 'Token Plan')} · 至 {period} · {renew}")

            month = ((data["usage"] or {}).get("monthUsage") or {})
            item = (month.get("items") or [{}])[0]
            used, limit = item.get("used", 0), item.get("limit", 0)
            pct = min(1.0, (used / limit) if limit else 0.0)
            bar_fg.config(width=int(bar_bg.winfo_width() * pct))
            usage_lbl.config(
                text=f"本月 {fmt_tokens(used)} / {fmt_tokens(limit)}（{pct:.0%}）")

            status_lbl.config(
                text=f"更新于 {time.strftime('%H:%M:%S', time.localtime(stamp))}",
                fg=DIM)
        root.after(2000, render)

    root.update_idletasks()
    root.geometry(f"+{root.winfo_screenwidth() - 300}+80")
    threading.Thread(target=worker, daemon=True).start()
    render()
    root.mainloop()


if __name__ == "__main__":
    import sys

    if "--fetch" in sys.argv:
        print(json.dumps(snapshot(Session()), ensure_ascii=False, indent=2))
    else:
        run_widget()
