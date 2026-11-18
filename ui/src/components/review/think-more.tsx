import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import type { AskThinking, Detail } from '@/lib/api'
import { detailLabel, setLiveDetail, useLiveDetail } from '@/lib/detail'

const SETTINGS = 'settings'

/**
 * Lo que se puede pedir en directo al modelo revisor junto a una petición: el
 * nivel de detalle (siempre) y «Pensar más» (solo si el modelo lo admite y no
 * piensa ya siempre).
 */
export function ThinkMore({ option, checked, onChange }: { option?: AskThinking; checked: boolean; onChange: (checked: boolean) => void }) {
  return (
    <span className="flex flex-wrap items-center gap-3">
      <DetailPicker />
      {option?.available && option.default !== true && (
        <label className="flex cursor-pointer items-center gap-2 text-xs text-muted-foreground" title="Activa el razonamiento del modelo solo para esta pregunta">
          <Switch size="sm" checked={checked} onCheckedChange={onChange} />
          Pensar más <span className="text-muted-foreground/80">(tarda más)</span>
        </label>
      )}
    </span>
  )
}

/** El nivel de detalle para lo que se pida desde ahora en esta ventana; por defecto, el de Ajustes. */
export function DetailPicker() {
  const live = useLiveDetail()
  return (
    <Select value={live ?? SETTINGS} onValueChange={(value) => setLiveDetail(value === SETTINGS ? null : (value as Detail))}>
      <SelectTrigger
        size="sm"
        className="h-7 gap-1 text-xs text-muted-foreground"
        aria-label="Nivel de detalle de la explicación"
        title="A quién va dirigida la explicación. Vale para lo que pidas desde ahora en esta ventana."
      >
        <span>Detalle:</span>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={SETTINGS}>El de Ajustes</SelectItem>
        {(Object.keys(detailLabel) as Detail[]).map((detail) => (
          <SelectItem key={detail} value={detail}>
            {detailLabel[detail]}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}
