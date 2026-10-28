import { cn } from '@/lib/utils'

/** La huella: arcos concéntricos, el exterior en el color de acento. */
export function Logo({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 26 26" fill="none" aria-hidden="true" className={cn('size-6', className)}>
      <path d="M13 3.5c5.2 0 9.5 4.3 9.5 9.5" stroke="var(--sidebar-primary)" strokeWidth="2" strokeLinecap="round" />
      <path d="M13 7.5c3 0 5.5 2.5 5.5 5.5v2" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
      <path d="M13 11.5c.8 0 1.5.7 1.5 1.5v5.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
      <path d="M9 13c0-2.2 1.8-4 4-4" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
      <path
        d="M5 13c0-1.6.5-3.1 1.3-4.4M9 16.5V13M5.4 17.5c-.3-1.4-.4-3-.4-4.5"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  )
}
