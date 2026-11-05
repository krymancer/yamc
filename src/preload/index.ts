import { contextBridge, ipcRenderer } from 'electron'
import type { CoreEvent } from '@shared/types'

contextBridge.exposeInMainWorld('yamc', {
  call: (method: string, ...args: unknown[]) => ipcRenderer.invoke('yamc:call', method, ...args),
  on: (cb: (e: CoreEvent) => void) => {
    const h = (_: unknown, e: CoreEvent) => cb(e)
    ipcRenderer.on('yamc:event', h)
    return () => ipcRenderer.off('yamc:event', h)
  },
})
