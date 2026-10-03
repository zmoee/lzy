/** 文件条目：由后端 /api/list → 蓝奏云 task=5 转换而来。
 *  status / progress 是纯前端状态（上传中行用），后端不返回。 */
export type LanzouFile = {
  id: string
  name: string
  ext: string
  bytes: number
  size?: string
  time: string
  downs: number
  locked: boolean
  status?: 'done' | 'uploading'
  progress?: number
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const units = ['K', 'M', 'G']
  let value = bytes / 1024
  let i = 0
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024
    i++
  }
  return `${value.toFixed(1)} ${units[i]}`
}
