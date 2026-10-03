import { useState, type FormEvent } from 'react'
import { AlertCircle, Cloud, Eye, EyeOff, Loader2 } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Input } from '@/components/ui/input'

/**
 * 登录卡：第一次打开、或者会话失效时显示。
 *
 * 以前账号密码是编在程序里的，谁拿到程序谁就有账号。现在改成用户自己输一次，
 * 后台用 Windows DPAPI 加密存在本机 —— 只有这台机器这个用户能解开。
 */
export function LoginCard({
  initialUser = '',
  expired = false,
  onSubmit,
}: {
  /** 预填的账号（上次用过的） */
  initialUser?: string
  /** 是不是"用着用着掉线了"（而不是第一次配置） */
  expired?: boolean
  onSubmit: (user: string, pwd: string, remember: boolean) => Promise<void>
}) {
  const [user, setUser] = useState(initialUser)
  const [pwd, setPwd] = useState('')
  const [remember, setRemember] = useState(true)
  const [showPwd, setShowPwd] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function submit(e: FormEvent) {
    e.preventDefault()
    if (busy) return
    setError(null)
    setBusy(true)
    try {
      await onSubmit(user.trim(), pwd, remember)
    } catch (err) {
      setError(err instanceof Error ? err.message : '登录失败')
      setPwd('')
    } finally {
      setBusy(false)
    }
  }

  return (
    <Card className="w-full max-w-sm">
      <CardContent className="space-y-5 py-2">
        <div className="flex flex-col items-center gap-2 pt-2 text-center">
          <div className="bg-muted flex size-11 items-center justify-center rounded-xl">
            <Cloud className="size-6" />
          </div>
          <div className="text-lg font-semibold">无限网盘</div>
          {/* 只有"掉线了"才给提示 —— 第一次用不需要解释，界面本身够清楚 */}
          {expired && (
            <div className="text-muted-foreground text-xs">登录状态已失效，请重新登录</div>
          )}
        </div>

        <form onSubmit={submit} className="space-y-3">
          <div className="space-y-1.5">
            <label htmlFor="lzy-user" className="text-xs font-medium">
              账号
            </label>
            <Input
              id="lzy-user"
              value={user}
              autoComplete="username"
              autoFocus={!initialUser}
              disabled={busy}
              onChange={(e) => setUser(e.target.value)}
              placeholder="手机号"
            />
          </div>

          <div className="space-y-1.5">
            <label htmlFor="lzy-pwd" className="text-xs font-medium">
              密码
            </label>
            <div className="relative">
              <Input
                id="lzy-pwd"
                type={showPwd ? 'text' : 'password'}
                value={pwd}
                autoComplete="current-password"
                autoFocus={!!initialUser}
                disabled={busy}
                onChange={(e) => setPwd(e.target.value)}
                placeholder="密码"
                className="pr-9"
              />
              <button
                type="button"
                tabIndex={-1}
                title={showPwd ? '隐藏' : '显示'}
                onClick={() => setShowPwd((v) => !v)}
                className="text-muted-foreground hover:text-foreground absolute top-2 right-2 flex size-6 items-center justify-center"
              >
                {showPwd ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
              </button>
            </div>
          </div>

          <label className="flex cursor-pointer items-center gap-2 text-xs select-none">
            <input
              type="checkbox"
              checked={remember}
              disabled={busy}
              onChange={(e) => setRemember(e.target.checked)}
              className="accent-primary size-3.5"
            />
            <span>记住密码（用 Windows 系统加密存在本机）</span>
          </label>

          {error && (
            <div className="border-destructive/30 bg-destructive/5 text-destructive flex items-start gap-2 rounded-md border px-3 py-2 text-xs">
              <AlertCircle className="mt-0.5 size-3.5 shrink-0" />
              <span className="flex-1 break-all">{error}</span>
            </div>
          )}

          <Button type="submit" className="w-full" disabled={busy || !user.trim() || !pwd}>
            {busy ? (
              <>
                <Loader2 className="size-4 animate-spin" />
                正在登录…
              </>
            ) : (
              '登录'
            )}
          </Button>
        </form>

        <p className="text-muted-foreground text-center text-[11px] leading-relaxed">
          账号密码只保存在这台电脑上
          {typeof window !== 'undefined' && <span className="hidden">（DPAPI）</span>}
          ，不会上传到任何地方。
        </p>
      </CardContent>
    </Card>
  )
}
