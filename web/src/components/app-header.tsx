import { useState, type ReactNode } from 'react'
import { Cloud, Copy, Minus, RefreshCw, Square, LogOut, X } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { closeWindow, minimizeWindow, toggleMaximizeWindow } from '@/lib/desktop'
import { cn } from '@/lib/utils'

/**
 * 应用头栏。
 *
 * 登录页和主界面共用同一个 —— 尤其是那三个窗口按钮：窗口是无边框的，
 * 没有它们就关不掉、也拖不动，登录页上会直接被卡住。
 *
 * `onRefresh` / `onSwitchAccount` 不给就不显示对应按钮（登录页就不给）。
 */
export function AppHeader({
  status,
  refreshing,
  onRefresh,
  onSwitchAccount,
}: {
  /** 左上角状态徽章的文字 */
  status: string
  refreshing?: boolean
  onRefresh?: () => void
  onSwitchAccount?: () => void
}) {
  const [maximized, setMaximized] = useState(false)

  return (
    <header className="app-drag bg-background/80 sticky top-0 z-10 border-b backdrop-blur">
      <div className="mx-auto flex h-14 max-w-6xl items-center justify-between gap-4 px-6">
        <div className="flex min-w-0 items-center gap-2">
          <Cloud className="size-5 shrink-0" />
          <span className="truncate font-semibold">无限网盘</span>
          <Badge variant="secondary" className="shrink-0">
            {status}
          </Badge>
        </div>

        <div className="app-no-drag flex shrink-0 items-center gap-0.5">
          {onRefresh && (
            <Button
              variant="ghost"
              size="icon"
              title="刷新"
              onClick={onRefresh}
              disabled={refreshing}
            >
              <RefreshCw className={refreshing ? 'size-4 animate-spin' : 'size-4'} />
            </Button>
          )}
          {onSwitchAccount && (
            <Button
              variant="ghost"
              size="icon"
              title="切换账号（会清掉本机记住的密码）"
              onClick={onSwitchAccount}
            >
              <LogOut className="size-4" />
            </Button>
          )}
          {(onRefresh || onSwitchAccount) && (
            <span className="bg-border mx-1.5 h-5 w-px" aria-hidden />
          )}

          <WindowButton title="最小化" onClick={() => void minimizeWindow()}>
            <Minus className="size-4" />
          </WindowButton>
          <WindowButton
            title={maximized ? '向下还原' : '最大化'}
            onClick={() => {
              void toggleMaximizeWindow()
              setMaximized((v) => !v)
            }}
          >
            {maximized ? <Copy className="size-3.5" /> : <Square className="size-3.5" />}
          </WindowButton>
          <WindowButton title="关闭" danger onClick={closeWindow}>
            <X className="size-4" />
          </WindowButton>
        </div>
      </div>
    </header>
  )
}

function WindowButton({
  title,
  onClick,
  danger,
  children,
}: {
  title: string
  onClick: () => void
  danger?: boolean
  children: ReactNode
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className={cn(
        'text-muted-foreground flex size-8 items-center justify-center rounded-md transition-colors',
        danger
          ? 'hover:bg-destructive hover:text-white'
          : 'hover:bg-accent hover:text-accent-foreground',
      )}
    >
      {children}
    </button>
  )
}
