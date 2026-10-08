// Tauri 桥：等价于原 Electron preload.js 暴露的 window.api
// 需在业务 <script> 之前加载
(function () {
  const T = window.__TAURI__;
  window.api = {
    getState: () => T.core.invoke('get_state'),
    onState: (cb) => T.event.listen('state', (e) => cb(e.payload)),
    saveConfig: (cfg) => T.core.invoke('save_config', { next: cfg }).catch(() => {}),
    refreshNow: () => T.core.invoke('refresh_now'),
    setFloatSize: (w, h) => T.core.invoke('set_float_size', { w, h }),
    copyText: (t) => T.core.invoke('copy_text', { t }),
  };

  // 拖拽：Tauri 用 data-tauri-drag-region 代替 -webkit-app-region: drag
  document.addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    if (e.target && e.target.closest && e.target.closest('[data-tauri-drag-region]')) {
      T.window.getCurrentWindow().startDragging();
    }
  });
})();
