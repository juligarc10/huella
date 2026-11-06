import { Switch } from '@/components/ui/switch'
import type { AskThinking } from '@/lib/api'

/** «Pensar más» en una pregunta: solo si el modelo lo admite y no piensa ya siempre. */
export function ThinkMore({ option, checked, onChange }: { option?: AskThinking; checked: boolean; onChange: (checked: boolean) => void }) {
  if (!option?.available || option.default === true) return null
  return (
    <label className="flex cursor-pointer items-center gap-2 text-xs text-muted-foreground" title="Activa el razonamiento del modelo solo para esta pregunta">
      <Switch size="sm" checked={checked} onCheckedChange={onChange} />
      Pensar más <span className="text-muted-foreground/80">(tarda más)</span>
    </label>
  )
}
