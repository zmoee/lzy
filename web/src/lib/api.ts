import type { LanzouFile } from '@/data/files'

export type LanzouFolder = {
  id: string
  name: string
  des: string
  /** 这个文件夹里装的是一套分卷，可以合并下载 */
  is_parts?: boolean
}
export type PathItem = { id: string; name: string }

/** 上传返回：单文件时给 file；超过单文件上限会被切成多块装进新文件夹（parts > 1）。 */
export type UploadResponse = {
  file?: LanzouFile
  folder?: LanzouFolder
  parts: number
}

/** 蓝奏云免费账号的单文件上限，与服务端保持一致 */
export const MAX_UPLOAD_BYTES = 100 * 1024 * 1024

export type ListResponse = {
  files: LanzouFile[]
  folders: LanzouFolder[]
  path: PathItem[]
  folder_id: number
  pages: number
  total: number
}

export type StatusResponse = {
  uid: string
  domain: string
  domains: { id: string; domain: string }[]
}

export type ShareResponse = { f_id: string; pwd: string; locked: boolean; url: string }

/** 带状态码的错误。401 意思是"该登录了"（没配置账号 或 会话失效）。 */
export class ApiError extends Error {
  readonly status: number
  constructor(message: string, status: number) {
    super(message)
    this.name = 'ApiError'
    this.status = status
  }
  get needsLogin() {
    return this.status === 401
  }
}

async function req<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, init)
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as { detail?: string } | null
    throw new ApiError(body?.detail ?? `请求失败 (${res.status})`, res.status)
  }
  return (await res.json()) as T
}

/** 会话状态：配过账号没有、用的哪个账号。 */
export type SessionResponse = { configured: boolean; user: string }

export const api = {
  /** 启动时问一句"配过账号没有"，据此决定弹不弹登录卡 */
  session: () => req<SessionResponse>('/api/session'),
  login: (user: string, pwd: string, remember: boolean) =>
    req<{ uid: string }>('/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ user, pwd, remember }),
    }),
  logout: (forget = false) =>
    req<{ ok: boolean }>(`/api/logout?forget=${forget}`, { method: 'POST' }),
  status: () => req<StatusResponse>('/api/status'),
  ping: () => req<{ ok: boolean }>('/api/ping'),
  list: (folderId = -1, pg = 0) =>
    req<ListResponse>(`/api/list?folder_id=${folderId}&pg=${pg}`),
  share: (fileId: string) => req<ShareResponse>(`/api/file/${fileId}/share`),
  remove: (fileId: string) => req<unknown>(`/api/file/${fileId}`, { method: 'DELETE' }),
  /** 删文件夹是递归的：文件夹和里面的文件一起进回收站（服务端实测如此） */
  removeFolder: (folderId: string) => req<unknown>(`/api/folder/${folderId}`, { method: 'DELETE' }),
  relogin: () => req<{ uid: string }>('/api/relogin', { method: 'POST' }),
}

/** 单文件下载走服务端代理：直链带 ESA 挑战且只经得起一次请求，浏览器直接开必踩失效页。 */
export const fileDownloadUrl = (file: LanzouFile) =>
  `/api/file/${file.id}/download?name=${encodeURIComponent(file.name)}`

/** 分卷文件夹的合并下载：服务端把所有块按序拼成一个响应，拿到手就是原文件。 */
export const folderDownloadUrl = (folder: LanzouFolder) =>
  `/api/folder/${folder.id}/download?name=${encodeURIComponent(folder.name)}`

/** 上传走 XHR：只有这样才能拿到真实的 upload.onprogress。 */
export function uploadFile(
  file: File,
  folderId = -1,
  onProgress?: (pct: number) => void,
): Promise<UploadResponse> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest()
    xhr.open('POST', `/api/upload?filename=${encodeURIComponent(file.name)}&folder_id=${folderId}`)
    xhr.setRequestHeader('Content-Type', 'application/octet-stream')
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable && onProgress) onProgress((e.loaded / e.total) * 100)
    }
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        try {
          resolve(JSON.parse(xhr.responseText) as UploadResponse)
        } catch {
          reject(new Error('响应解析失败'))
        }
        return
      }
      let detail = `上传失败 (${xhr.status})`
      try {
        detail = (JSON.parse(xhr.responseText) as { detail?: string }).detail ?? detail
      } catch {
        /* 保留默认信息 */
      }
      reject(new Error(detail))
    }
    xhr.onerror = () => reject(new Error('网络错误，上传中断'))
    xhr.send(file)
  })
}
