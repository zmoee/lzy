import { useRef, useState } from 'react'
import { Upload } from 'lucide-react'

import { cn } from '@/lib/utils'

/** 拖拽 / 点选上传区。当前只把选中的文件交给上层，不发请求。 */
export function FileUploader({ onPick, disabled }: { onPick: (files: File[]) => void; disabled?: boolean }) {
  const [over, setOver] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)

  return (
    <div
      onClick={() => !disabled && inputRef.current?.click()}
      onDragOver={(e) => {
        e.preventDefault()
        setOver(true)
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        e.preventDefault()
        setOver(false)
        if (disabled) return
        const files = Array.from(e.dataTransfer.files)
        if (files.length) onPick(files)
      }}
      className={cn(
        'flex cursor-pointer flex-col items-center justify-center gap-2 rounded-xl border border-dashed px-6 py-8 text-center transition-colors',
        over ? 'border-primary bg-primary/5' : 'border-border hover:border-primary/50 hover:bg-muted/40',
        disabled && 'pointer-events-none opacity-60',
      )}
    >
      <div className="bg-muted flex size-10 items-center justify-center rounded-full">
        <Upload className="text-muted-foreground size-5" />
      </div>
      <div className="text-sm font-medium">把文件拖到这里上传</div>
      <div className="text-muted-foreground text-xs">
        或点击选择文件 · 超过 100 M 会自动切块，装进一个同名文件夹里
      </div>
      <input
        ref={inputRef}
        type="file"
        multiple
        className="hidden"
        onChange={(e) => {
          const files = Array.from(e.target.files ?? [])
          if (files.length) onPick(files)
          e.target.value = ''
        }}
      />
    </div>
  )
}
