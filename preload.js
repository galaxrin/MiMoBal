'use strict';
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('api', {
  getState: () => ipcRenderer.invoke('get-state'),
  onState: (cb) => ipcRenderer.on('state', (_e, s) => cb(s)),
  saveConfig: (cfg) => ipcRenderer.send('save-config', cfg),
  setFloatSize: (w, h) => ipcRenderer.send('set-float-size', w, h),
  copyText: (t) => ipcRenderer.send('copy-text', t),
});
