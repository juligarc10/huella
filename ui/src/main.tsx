import './index.css'

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { createHashRouter, RouterProvider } from 'react-router'

import { AppShell } from '@/components/app-shell'
import { RouteError } from '@/components/route-error'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import { ApiError } from '@/lib/api'
import { ProjectProvider } from '@/lib/project'
import { ThemeProvider } from '@/lib/theme'
import { DecisionsPage } from '@/pages/decisions'
import { ReviewPage } from '@/pages/review'
import { SearchPage } from '@/pages/search'
import { SessionsPage } from '@/pages/sessions'
import { StatusPage } from '@/pages/status'
import { SymbolPage } from '@/pages/symbol'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      // Sin token no tiene sentido reintentar.
      retry: (count, error) => !(error instanceof ApiError && error.status < 500) && count < 2,
    },
  },
})

// Rutas en el fragmento (#/decisiones): el servidor solo sirve una página.
const router = createHashRouter([
  {
    path: '/',
    element: <AppShell />,
    errorElement: <RouteError />,
    children: [
      { index: true, element: <SearchPage /> },
      { path: 'decisiones', element: <DecisionsPage /> },
      { path: 'simbolo', element: <SymbolPage /> },
      { path: 'sesiones', element: <SessionsPage /> },
      { path: 'sesiones/:session', element: <ReviewPage /> },
      { path: 'estado', element: <StatusPage /> },
    ],
  },
])

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <ThemeProvider>
      <ProjectProvider>
        <QueryClientProvider client={queryClient}>
          <TooltipProvider delayDuration={300}>
            <RouterProvider router={router} />
            <Toaster richColors position="bottom-right" />
          </TooltipProvider>
        </QueryClientProvider>
      </ProjectProvider>
    </ThemeProvider>
  </StrictMode>,
)
