import { Fragment, useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import {
  AlertCircle,
  ChevronRight,
  FileText,
  FolderOpen,
  Globe,
  HardDrive,
  Loader2,
  Search,
} from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { FileTable } from '@/components/file-table'
import { FileUploader } from '@/components/file-uploader'
import { LoginCard } from '@/components/login-card'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { formatBytes, type LanzouFile } from '@/data/files'
import {
  ApiError,
  MAX_UPLOAD_BYTES,
  api,
  uploadFile,
  type LanzouFolder,
  type PathItem,
  type SessionResponse,
} from '@/lib/api'

const ROOT = -1

let seq = 0

function extOf(name: string) {
  return name.includes('.') ? (name.split('.').pop() as string).toLowerCase() : 'file'
}

function StatCard({ label, value, hint, icon }: { label: string; value: string; hint: string; icon: ReactNode }) {
  return (
    <Card className="gap-2 py-4">
      <CardContent className="flex items-start justify-between">
        <div className="min-w-0 space-y-1">
          <div className="text-muted-foreground text-xs">{label}</div>
          <div className="truncate text-xl font-semibold tracking-tight">{value}</div>
          <div className="text-muted-foreground truncate text-xs" title={hint}>
            {hint}
          </div>
        </div>
        <div className="bg-muted flex size-8 shrink-0 items-center justify-center rounded-lg">{icon}</div>
      </CardContent>
    </Card>
  )
}


export default function App() {
  const [files, setFiles] = useState<LanzouFile[]>([])
  const [folders, setFolders] = useState<LanzouFolder[]>([])
  const [path, setPath] = useState<PathItem[]>([])
  const [folderId, setFolderId] = useState<number>(ROOT)
  const [pages, setPages] = useState(0)
  const [domain, setDomain] = useState('')
  const [uid, setUid] = useState('')
  const [query, setQuery] = useState('')
  // 'checking' 启动时问会话状态；'login' 该弹登录卡；'ready' 正常用
  const [phase, setPhase] = useState<'checking' | 'login' | 'ready'>('checking')
  const [sessionUser, setSessionUser] = useState('')
  const [sessionExpired, setSessionExpired] = useState(false)
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState<string | null>(null)

  /** 任何调用碰到 401 都回到登录卡。返回 true 表示已经处理掉了。 */
  const handleAuthError = useCallback((err: unknown) => {
    if (err instanceof ApiError && err.needsLogin) {
      setSessionExpired(true)
      setPhase('login')
      return true
    }
    return false
  }, [])

  const load = useCallback(async (target: number) => {
    const list = await api.list(target)
    setFiles(list.files.map((file) => ({ ...file, status: 'done' as const, progress: 100 })))
    setFolders(list.folders)
    setPath(list.path)
    setPages(list.pages)
    setFolderId(list.folder_id)
  }, [])

  /** 进主界面之后拉账号信息 + 根目录 */
  const boot = useCallback(async () => {
    try {
      const [info] = await Promise.all([api.status(), load(ROOT)])
      setUid(info.uid)
      setDomain(info.domain)
    } catch (err) {
      if (handleAuthError(err)) return
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [load, handleAuthError])

  // 启动：先问一句配过账号没有，据此决定弹登录卡还是直接进
  useEffect(() => {
    ;(async () => {
      let s: SessionResponse | null = null
      try {
        s = await api.session()
      } catch {
        // 后端没起来之类，让下面的 status() 去报真正的错
      }
      if (!s?.configured) {
        setSessionUser(s?.user ?? '')
        setPhase('login')
        setLoading(false)
        return
      }
      setSessionUser(s.user)
      setPhase('ready')
      await boot()
    })()
  }, [boot, handleAuthError])

  /** 登录卡提交 */
  const handleLogin = useCallback(
    async (user: string, pwd: string, remember: boolean) => {
      await api.login(user, pwd, remember) // 失败会抛，错误显示在登录卡里
      setSessionUser(user)
      setSessionExpired(false)
      setLoading(true)
      setFiles([])
      setFolders([])
      setPath([])
      setFolderId(ROOT)
      setPhase('ready')
      await boot()
    },
    [boot],
  )

  /** 切换账号：把存盘的凭据也抹掉，退回登录卡 */
  const handleSwitchAccount = useCallback(async () => {
    await api.logout(true).catch(() => undefined)
    setFiles([])
    setFolders([])
    setPath([])
    setUid('')
    setDomain('')
    setError(null)
    setSessionExpired(false)
    setPhase('login')
  }, [])

  useEffect(() => {
    // 心跳：告诉本地服务"窗口还开着"，别被闲置自退逻辑收走
    const timer = setInterval(() => {
      void api.ping().catch(() => undefined)
    }, 45000)
    return () => clearInterval(timer)
  }, [])

  const enterFolder = useCallback(
    async (target: number | string) => {
      const id = Number(target)
      setLoading(true)
      setError(null)
      setQuery('')
      try {
        await load(id)
      } catch (err) {
        if (handleAuthError(err)) return
        setError(err instanceof Error ? err.message : String(err))
      } finally {
        setLoading(false)
      }
    },
    [load, handleAuthError],
  )

  async function handleRefresh() {
    setRefreshing(true)
    setError(null)
    try {
      await load(folderId)
    } catch (err) {
      if (handleAuthError(err)) return
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setRefreshing(false)
    }
  }

  async function handlePick(picked: File[]) {
    setError(null)
    for (const file of picked) {
      // 超过单文件上限的会被服务端切成多块、装进一个新的同名文件夹
      const willSplit = file.size > MAX_UPLOAD_BYTES
      const tempId = `local-${Date.now()}-${seq++}`
      setFiles((prev) => [
        {
          id: tempId,
          name: file.name,
          ext: extOf(file.name),
          bytes: file.size,
          time: willSplit ? '分块上传中' : '上传中',
          downs: 0,
          locked: false,
          status: 'uploading',
          progress: 0,
        },
        ...prev,
      ])
      try {
        const res = await uploadFile(file, folderId, (pct) => {
          setFiles((prev) =>
            prev.map((item) =>
              item.id === tempId
                ? {
                    ...item,
                    progress: pct,
                    // 进度条量的是"传到本机服务"这一段。大文件传完之后服务端还要
                    // 逐块转发到网盘，那段时间进度条一直是满的，得把话说清楚。
                    time: willSplit && pct >= 99.5 ? '逐块上传到网盘…' : item.time,
                  }
                : item,
            ),
          )
        })

        if (res.parts > 1) {
          // 大文件被切成多块装进了新文件夹：临时行删掉，重新拉目录让文件夹显示出来
          setFiles((prev) => prev.filter((item) => item.id !== tempId))
          await load(folderId)
          continue
        }
        const uploaded = res.file
        if (!uploaded) throw new Error('服务端没有返回文件信息')
        setFiles((prev) =>
          prev.map((item) => (item.id === tempId ? { ...uploaded, status: 'done' as const, progress: 100 } : item)),
        )
      } catch (err) {
        setFiles((prev) => prev.filter((item) => item.id !== tempId))
        setError(err instanceof Error ? err.message : '上传失败')
      }
    }
  }

  async function handleCopy(file: LanzouFile) {
    try {
      const share = await api.share(file.id)
      return share.url || null
    } catch (err) {
      setError(err instanceof Error ? err.message : '获取分享链接失败')
      return null
    }
  }

  async function handleDeleteFolder(folder: LanzouFolder) {
    if (!window.confirm(`删除文件夹「${folder.name}」？\n里面的文件会一起删除（进回收站）。`)) return
    setError(null)
    try {
      await api.removeFolder(folder.id)
      setFolders((prev) => prev.filter((item) => item.id !== folder.id))
    } catch (err) {
      setError(err instanceof Error ? err.message : '删除文件夹失败')
    }
  }

  async function handleDelete(id: string) {
    try {
      await api.remove(id)
      setFiles((prev) => prev.filter((file) => file.id !== id))
    } catch (err) {
      setError(err instanceof Error ? err.message : '删除失败')
    }
  }

  const matched = useMemo(() => {
    const keyword = query.trim().toLowerCase()
    return keyword ? files.filter((file) => file.name.toLowerCase().includes(keyword)) : files
  }, [files, query])

  const totalBytes = useMemo(
    () => files.filter((file) => file.status !== 'uploading').reduce((sum, file) => sum + file.bytes, 0),
    [files],
  )

  const inRoot = folderId === ROOT

  // 还没问出会话状态
  if (phase === 'checking') {
    return (
      <div className="bg-background flex min-h-screen items-center justify-center">
        <Loader2 className="text-muted-foreground size-5 animate-spin" />
      </div>
    )
  }

  // 没配账号、或者掉线了 —— 主界面不渲染，但**头栏必须有**：
  // 窗口是无边框的，没有那三个按钮就关不掉也拖不动，会把人卡在这儿
  if (phase === 'login') {
    return (
      <div className="bg-background flex min-h-screen flex-col">
        <AppHeader status="未登录" />
        <main className="flex flex-1 items-center justify-center p-6">
          <LoginCard initialUser={sessionUser} expired={sessionExpired} onSubmit={handleLogin} />
        </main>
      </div>
    )
  }

  return (
    <div className="bg-background min-h-screen">
      <AppHeader
        status={uid ? `已连接 · ${uid}` : '连接中'}
        refreshing={refreshing}
        onRefresh={handleRefresh}
        onSwitchAccount={() => void handleSwitchAccount()}
      />

      <main className="mx-auto max-w-6xl space-y-5 px-6 py-6">
        {error && (
          <div className="border-destructive/30 bg-destructive/5 text-destructive flex items-start gap-2 rounded-lg border px-4 py-3 text-sm">
            <AlertCircle className="mt-0.5 size-4 shrink-0" />
            <span className="flex-1">{error}</span>
            <button className="text-xs underline" onClick={() => setError(null)}>
              关闭
            </button>
          </div>
        )}

        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          <StatCard
            label={inRoot ? '文件总数' : '当前目录文件'}
            value={loading ? '—' : String(files.length)}
            hint={pages > 1 ? `已合并 ${pages} 页 · 筛选出 ${matched.length} 条` : `筛选出 ${matched.length} 条`}
            icon={<FileText className="text-muted-foreground size-4" />}
          />
          <StatCard
            label="占用空间"
            value={loading ? '—' : formatBytes(totalBytes)}
            hint={inRoot ? '根目录已用' : '当前目录已用'}
            icon={<HardDrive className="text-muted-foreground size-4" />}
          />
          <StatCard
            label="子文件夹"
            value={loading ? '—' : String(folders.length)}
            hint={folders.map((f) => f.name).join(' · ') || '无'}
            icon={<FolderOpen className="text-muted-foreground size-4" />}
          />
          <StatCard
            label="分享域名"
            value={domain || '—'}
            hint="自定义域名已生效"
            icon={<Globe className="text-muted-foreground size-4" />}
          />
        </div>

        <FileUploader onPick={handlePick} />

        <Card className="gap-0 py-0">
          <div className="flex flex-wrap items-center justify-between gap-3 border-b px-5 py-3">
            <nav className="flex items-center gap-1 text-sm">
              <button
                onClick={() => !inRoot && enterFolder(ROOT)}
                className={
                  inRoot
                    ? 'font-medium'
                    : 'text-muted-foreground hover:text-foreground underline-offset-4 hover:underline'
                }
              >
                根目录
              </button>
              {path.map((item, index) => (
                <Fragment key={item.id}>
                  <ChevronRight className="text-muted-foreground size-3.5" />
                  <button
                    onClick={() => enterFolder(item.id)}
                    className={
                      index === path.length - 1
                        ? 'font-medium'
                        : 'text-muted-foreground hover:text-foreground underline-offset-4 hover:underline'
                    }
                  >
                    {item.name}
                  </button>
                </Fragment>
              ))}
              <Badge variant="secondary" className="ml-2">
                {folders.length + matched.length}
              </Badge>
            </nav>
            <div className="relative w-64">
              <Search className="text-muted-foreground pointer-events-none absolute top-2.5 left-2.5 size-4" />
              <Input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="搜索当前目录文件名"
                className="pl-8"
              />
            </div>
          </div>
          {loading ? (
            <div className="text-muted-foreground flex h-32 items-center justify-center text-sm">
              正在从蓝奏云拉取文件列表…
            </div>
          ) : (
            <FileTable
              folders={folders}
              files={matched}
              onEnter={enterFolder}
              onDelete={handleDelete}
              onDeleteFolder={handleDeleteFolder}
              onCopy={handleCopy}
            />
          )}
        </Card>
      </main>
    </div>
  )
}
