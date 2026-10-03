import { useState } from 'react'
import { Check, ChevronRight, Copy, Download, Folder, Lock, Package, Trash2 } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { formatBytes, type LanzouFile } from '@/data/files'
import { fileDownloadUrl, folderDownloadUrl, type LanzouFolder } from '@/lib/api'

const EXT_TONE: Record<string, string> = {
  zip: 'border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-500/30 dark:bg-amber-500/10 dark:text-amber-400',
  '7z': 'border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-500/30 dark:bg-amber-500/10 dark:text-amber-400',
  docx: 'border-sky-200 bg-sky-50 text-sky-700 dark:border-sky-500/30 dark:bg-sky-500/10 dark:text-sky-400',
  iso: 'border-violet-200 bg-violet-50 text-violet-700 dark:border-violet-500/30 dark:bg-violet-500/10 dark:text-violet-400',
}

function extTone(ext: string) {
  return EXT_TONE[ext] ?? 'border-border bg-muted text-muted-foreground'
}

export function FileTable({
  folders,
  files,
  onEnter,
  onDelete,
  onDeleteFolder,
  onCopy,
}: {
  folders: LanzouFolder[]
  files: LanzouFile[]
  onEnter: (folderId: string) => void
  onDelete: (id: string) => void
  onDeleteFolder: (folder: LanzouFolder) => void
  onCopy: (file: LanzouFile) => Promise<string | null>
}) {
  const [copied, setCopied] = useState<string | null>(null)

  async function handleCopy(file: LanzouFile) {
    const url = await onCopy(file)
    if (!url) return
    try {
      await navigator.clipboard.writeText(url)
    } catch {
      // 非 https 等场景剪贴板不可用，仅给出视觉反馈
    }
    setCopied(file.id)
    setTimeout(() => setCopied((cur) => (cur === file.id ? null : cur)), 1500)
  }

  function handleDownload(file: LanzouFile) {
    // 下载走服务端代理：直链带 ESA 挑战且只经得起一次请求，浏览器直接开必踩失效页
    triggerDownload(fileDownloadUrl(file), file.name)
  }

  function handleDownloadFolder(folder: LanzouFolder) {
    // 分卷文件夹：服务端把各块按序拼成一个响应，这里就是一次普通下载
    triggerDownload(folderDownloadUrl(folder), folder.name)
  }

  function triggerDownload(url: string, name: string) {
    const link = document.createElement('a')
    link.href = url
    link.download = name
    document.body.appendChild(link)
    link.click()
    link.remove()
  }

  const empty = folders.length === 0 && files.length === 0

  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>名称</TableHead>
          <TableHead className="w-24">大小</TableHead>
          <TableHead className="w-28">时间</TableHead>
          <TableHead className="w-20 text-right">下载</TableHead>
          <TableHead className="w-28 text-right">操作</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {empty ? (
          <TableRow>
            <TableCell colSpan={5} className="text-muted-foreground h-28 text-center">
              这个文件夹是空的
            </TableCell>
          </TableRow>
        ) : (
          <>
            {folders.map((folder) => (
              <TableRow
                key={`folder-${folder.id}`}
                className="cursor-pointer"
                onClick={() => onEnter(folder.id)}
              >
                <TableCell>
                  <div className="flex min-w-0 items-center gap-2">
                    <Folder className="size-4 shrink-0 fill-amber-400/30 text-amber-500" />
                    <span className="truncate font-medium" title={folder.name}>
                      {folder.name}
                    </span>
                    {folder.is_parts ? (
                      <Badge
                        variant="outline"
                        className="border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-500/30 dark:bg-emerald-500/10 dark:text-emerald-400"
                      >
                        <Package className="size-3" />
                        分卷
                      </Badge>
                    ) : (
                      <Badge variant="outline" className="text-muted-foreground">
                        文件夹
                      </Badge>
                    )}
                    {folder.des && (
                      <span className="text-muted-foreground truncate text-xs">
                        {folder.is_parts ? '点右侧按钮可合并下载' : folder.des}
                      </span>
                    )}
                  </div>
                </TableCell>
                <TableCell className="text-muted-foreground">—</TableCell>
                <TableCell className="text-muted-foreground">—</TableCell>
                <TableCell className="text-muted-foreground text-right">—</TableCell>
                <TableCell className="text-right">
                  <div className="flex justify-end gap-1">
                    {folder.is_parts && (
                      <Button
                        variant="ghost"
                        size="icon"
                        title="合并下载（把里面各块拼回原文件）"
                        onClick={(e) => {
                          e.stopPropagation()
                          handleDownloadFolder(folder)
                        }}
                      >
                        <Download className="size-4" />
                      </Button>
                    )}
                    <Button
                      variant="ghost"
                      size="icon"
                      title="删除文件夹（连同里面的文件）"
                      className="text-muted-foreground hover:text-destructive"
                      onClick={(e) => {
                        e.stopPropagation()
                        onDeleteFolder(folder)
                      }}
                    >
                      <Trash2 className="size-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      title="打开文件夹"
                      onClick={(e) => {
                        e.stopPropagation()
                        onEnter(folder.id)
                      }}
                    >
                      <ChevronRight className="size-4" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}

            {files.map((file) => (
              <TableRow key={file.id}>
                <TableCell>
                  <div className="flex min-w-0 items-center gap-2">
                    <Badge variant="outline" className={extTone(file.ext)}>
                      {file.ext.toUpperCase()}
                    </Badge>
                    <span className="truncate" title={file.name}>
                      {file.name}
                    </span>
                    {file.locked && <Lock className="text-muted-foreground size-3.5 shrink-0" />}
                  </div>
                  {file.status === 'uploading' && (
                    <div className="mt-2 flex items-center gap-2">
                      <Progress value={file.progress ?? 0} className="max-w-48" />
                      <span className="text-muted-foreground text-xs tabular-nums">
                        {Math.floor(file.progress ?? 0)}%
                      </span>
                    </div>
                  )}
                </TableCell>
                <TableCell className="text-muted-foreground tabular-nums">
                  {formatBytes(file.bytes)}
                </TableCell>
                <TableCell className="text-muted-foreground">{file.time}</TableCell>
                <TableCell className="text-right tabular-nums">{file.downs}</TableCell>
                <TableCell className="text-right">
                  <div className="flex justify-end gap-1">
                    <Button
                      variant="ghost"
                      size="icon"
                      title="下载"
                      disabled={file.status === 'uploading'}
                      onClick={() => handleDownload(file)}
                    >
                      <Download className="size-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      title="复制分享链接"
                      disabled={file.status === 'uploading'}
                      onClick={() => handleCopy(file)}
                    >
                      {copied === file.id ? (
                        <Check className="size-4 text-emerald-600" />
                      ) : (
                        <Copy className="size-4" />
                      )}
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      title="删除"
                      className="text-muted-foreground hover:text-destructive"
                      disabled={file.status === 'uploading'}
                      onClick={() => onDelete(file.id)}
                    >
                      <Trash2 className="size-4" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </>
        )}
      </TableBody>
    </Table>
  )
}
