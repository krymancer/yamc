import type { YamcApi } from '@shared/api'
import type { CoreEvent } from '@shared/types'

type Bridge = { call(method: string, ...args: unknown[]): Promise<unknown>; on(cb: (e: CoreEvent) => void): () => void }
const bridge = () => (globalThis as unknown as { yamc: Bridge }).yamc

type Async<T> = { [K in keyof T]: T[K] extends (...a: infer A) => infer R ? (...a: A) => Promise<Awaited<R>> : never }

export const api = new Proxy({} as Async<YamcApi>, {
  get: (_t, method: string) => async (...args: unknown[]) => {
    try {
      return await bridge().call(method, ...args)
    } catch (err) {
      // Strip Electron's "Error invoking remote method ..." prefix.
      const msg = err instanceof Error ? err.message : String(err)
      throw new Error(msg.replace(/^Error invoking remote method '[^']+': (Error: )?/, ''))
    }
  },
})

export const onEvent = (cb: (e: CoreEvent) => void) => bridge().on(cb)
