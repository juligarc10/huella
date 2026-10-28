import { TriangleAlert } from 'lucide-react'
import { isRouteErrorResponse, useRouteError } from 'react-router'

import { Button } from '@/components/ui/button'

/** Lo que se ve si una pantalla falla: el motivo y una salida, no una traza. */
export function RouteError() {
  const error = useRouteError()
  const message = isRouteErrorResponse(error)
    ? `${error.status} ${error.statusText}`
    : error instanceof Error
      ? error.message
      : String(error)
  return (
    <div className="flex min-h-svh items-center justify-center bg-background p-6 text-foreground">
      <div className="flex max-w-md flex-col items-center gap-4 text-center">
        <TriangleAlert className="size-9 text-warn" aria-hidden="true" />
        <h1 className="text-xl font-semibold">Algo ha fallado en esta pantalla</h1>
        <p className="font-mono text-sm break-words text-muted-foreground">{message}</p>
        <Button onClick={() => window.location.assign('/#/')}>Volver a Buscar</Button>
      </div>
    </div>
  )
}
