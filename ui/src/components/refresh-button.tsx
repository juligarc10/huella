import { useIsFetching, useQueryClient, type QueryKey } from '@tanstack/react-query'
import { RefreshCw } from 'lucide-react'
import { useState } from 'react'

import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

/**
 * Vuelve a pedir lo que se ve: las consultas de `keys`, o lo que haga `onRefresh`.
 * Gira mientras tanto y avisa a los lectores de pantalla al acabar.
 */
export function RefreshButton({
  keys = [],
  onRefresh,
  refreshing,
  label = 'Refrescar',
  className,
}: {
  keys?: QueryKey[]
  onRefresh?: () => Promise<unknown> | void
  refreshing?: boolean
  label?: string
  className?: string
}) {
  const client = useQueryClient()
  const [running, setRunning] = useState(false)
  const fetching = useIsFetching({ predicate: (query) => keys.some((key) => key.every((part, index) => query.queryKey[index] === part)) }) > 0
  const busy = refreshing ?? (running || (keys.length > 0 && fetching))
  const [done, setDone] = useState('')

  const refresh = async () => {
    setRunning(true)
    setDone('')
    try {
      await Promise.all([onRefresh?.(), ...keys.map((queryKey) => client.invalidateQueries({ queryKey }))])
      setDone('Actualizado')
    } finally {
      setRunning(false)
    }
  }

  return (
    <>
      <Button variant="outline" className={cn('min-h-11', className)} onClick={refresh} disabled={busy} aria-busy={busy}>
        <RefreshCw className={cn('motion-safe:transition-transform', busy && 'motion-safe:animate-spin')} aria-hidden="true" />
        {busy ? 'Actualizando…' : label}
      </Button>
      <span className="sr-only" role="status" aria-live="polite">
        {done}
      </span>
    </>
  )
}
