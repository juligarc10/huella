import { createContext, useContext, useMemo, useState, type ReactNode } from 'react'

// Proyecto elegido en la barra lateral; `null` = todos. Se recuerda entre visitas.

interface ProjectValue {
  project: string | null
  setProject: (project: string | null) => void
}

const ProjectContext = createContext<ProjectValue | null>(null)
const KEY = 'huella-project'

export function ProjectProvider({ children }: { children: ReactNode }) {
  const [project, setProjectState] = useState<string | null>(() => {
    try {
      return localStorage.getItem(KEY)
    } catch {
      return null
    }
  })
  const value = useMemo<ProjectValue>(
    () => ({
      project,
      setProject: (next) => {
        setProjectState(next)
        try {
          if (next) localStorage.setItem(KEY, next)
          else localStorage.removeItem(KEY)
        } catch {
          // Sin almacenamiento, la elección dura lo que la pestaña.
        }
      },
    }),
    [project],
  )
  return <ProjectContext.Provider value={value}>{children}</ProjectContext.Provider>
}

export function useProject(): ProjectValue {
  const value = useContext(ProjectContext)
  if (!value) throw new Error('useProject fuera de ProjectProvider')
  return value
}
