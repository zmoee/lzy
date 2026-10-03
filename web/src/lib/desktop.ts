/**
 * 窗口控制抽象。
 *
 * 桌面壳按优先级注入：
 *   1. window.desktop  —— Electron preload / Tauri 自己挂的桥（推荐）
 *   2. window.__TAURI__.window —— Tauri 开启 withGlobalTauri 时的全局对象
 * 两者都没有（即纯浏览器预览）时降级：最大化切全屏，关闭调 window.close()，
 * 最小化在浏览器里没有对应行为，静默忽略。
 */
export type DesktopBridge = {
  minimize?: () => void | Promise<void>
  toggleMaximize?: () => void | Promise<void>
  close?: () => void | Promise<void>
}

type TauriWindowLike = {
  minimize: () => Promise<void>
  toggleMaximize: () => Promise<void>
  close: () => Promise<void>
}

declare global {
  interface Window {
    desktop?: DesktopBridge
    __TAURI__?: { window?: { getCurrentWindow?: () => TauriWindowLike } }
  }
}

function resolveBridge(): DesktopBridge | null {
  if (typeof window === 'undefined') return null
  if (window.desktop) return window.desktop
  const win = window.__TAURI__?.window?.getCurrentWindow?.()
  if (!win) return null
  return {
    minimize: () => win.minimize(),
    toggleMaximize: () => win.toggleMaximize(),
    close: () => win.close(),
  }
}

export async function minimizeWindow(): Promise<void> {
  await resolveBridge()?.minimize?.()
}

export async function toggleMaximizeWindow(): Promise<void> {
  const bridge = resolveBridge()
  if (bridge?.toggleMaximize) {
    await bridge.toggleMaximize()
    return
  }
  if (document.fullscreenElement) await document.exitFullscreen().catch(() => undefined)
  else await document.documentElement.requestFullscreen().catch(() => undefined)
}

export function closeWindow(): void {
  const bridge = resolveBridge()
  if (bridge?.close) {
    void bridge.close()
    return
  }
  window.close() // 浏览器多半静默拒绝，属于预期内的降级
}
