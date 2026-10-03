import { cn } from '@/lib/utils'

function Progress({ value = 0, className }: { value?: number; className?: string }) {
  return (
    <div className={cn('bg-primary/15 h-1.5 w-full overflow-hidden rounded-full', className)}>
      <div
        className="bg-primary h-full rounded-full transition-all duration-200 ease-out"
        style={{ width: `${Math.min(100, Math.max(0, value))}%` }}
      />
    </div>
  )
}

export { Progress }
