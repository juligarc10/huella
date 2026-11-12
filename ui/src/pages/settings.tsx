import { CheckCircle2, CircleAlert, KeyRound, Languages, Loader2, PlugZap } from 'lucide-react'
import { useEffect, useState, type ReactNode } from 'react'

import { Mono, PageHeader, Panel } from '@/components/bits'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import type { Settings, SettingsChange } from '@/lib/api'
import { useSaveKey, useSaveSettings, useSettings, useTestModel } from '@/lib/queries'

type Provider = 'anthropic' | 'openai-compatible'
type Thinking = 'server' | 'on' | 'off'

interface Form {
  provider: Provider
  base_url: string
  model: string
  context_tokens: string
  thinking: Thinking
}

const thinkingOf = (value: boolean | null): Thinking => (value === null ? 'server' : value ? 'on' : 'off')

function formOf(settings: Settings): Form {
  return {
    provider: settings.provider.value,
    base_url: settings.base_url.value ?? '',
    model: settings.model.value ?? '',
    context_tokens: String(settings.context_tokens.value),
    thinking: thinkingOf(settings.thinking.value),
  }
}

/** Solo lo que ha cambiado y se puede cambiar aquí (lo que fija una variable de entorno, no). */
function changes(settings: Settings, form: Form): SettingsChange {
  const before = formOf(settings)
  const out: SettingsChange = {}
  if (form.provider !== before.provider && settings.provider.from !== 'env') out.provider = form.provider
  if (form.base_url.trim() !== before.base_url && settings.base_url.from !== 'env') out.base_url = form.base_url.trim() || null
  if (form.model.trim() !== before.model && settings.model.from !== 'env') out.model = form.model.trim() || null
  if (form.thinking !== before.thinking) out.thinking = form.thinking === 'server' ? null : form.thinking === 'on'
  if (form.context_tokens.trim() !== before.context_tokens) out.context_tokens = form.context_tokens.trim() ? Number(form.context_tokens) : null
  return out
}

export function SettingsPage() {
  const { data: settings } = useSettings()
  const save = useSaveSettings()
  const test = useTestModel()
  const [form, setForm] = useState<Form | null>(null)
  // El formulario parte de lo guardado, y vuelve a ello tras guardar.
  useEffect(() => {
    if (settings && (!form || save.isSuccess)) setForm(formOf(settings))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings, save.isSuccess])

  if (!settings || !form) return <Skeleton className="h-96 rounded-2xl" />
  const pending = changes(settings, form)
  const dirty = Object.keys(pending).length > 0
  const local = form.provider === 'openai-compatible'
  const set = (patch: Partial<Form>) => setForm({ ...form, ...patch })
  const probe = settings.probe
  const saveAndTest = () => {
    if (dirty) save.mutate(pending, { onSuccess: () => test.mutate() })
    else test.mutate()
  }

  return (
    <>
      <PageHeader
        title="Ajustes"
        description={
          <>
            El modelo que escribe las guías y responde tus preguntas. Se guarda en <Mono>{settings.path ?? 'config.toml'}</Mono> y se aplica al
            momento.
          </>
        }
      />

      {settings.invalid && <Notice tone="warn">{settings.invalid}</Notice>}
      {settings.problem && <Notice tone="warn">{settings.problem}</Notice>}

      <Panel className="flex flex-col gap-6">
        <h2 className="text-sm font-semibold">Modelo</h2>

        <Field label="Proveedor" env={settings.provider}>
          <ToggleGroup
            type="single"
            variant="outline"
            value={form.provider}
            onValueChange={(value) => value && set({ provider: value as Provider })}
            disabled={settings.provider.from === 'env'}
            aria-label="Proveedor"
            className="flex-wrap justify-start"
          >
            <ToggleGroupItem value="openai-compatible" className="px-3">
              Servidor compatible con OpenAI
            </ToggleGroupItem>
            <ToggleGroupItem value="anthropic" className="px-3">
              Claude (Anthropic)
            </ToggleGroupItem>
          </ToggleGroup>
          <p className="text-xs text-muted-foreground">
            {local ? 'llama.cpp, Ollama, LM Studio, vLLM… en tu máquina o en tu servidor.' : 'La API de Claude, con tu clave de Anthropic.'}
          </p>
        </Field>

        <div className="grid gap-6 md:grid-cols-2">
          <Field label="URL de la API" id="ajuste-url" env={settings.base_url}>
            <Input
              id="ajuste-url"
              value={form.base_url}
              onChange={(event) => set({ base_url: event.target.value })}
              disabled={settings.base_url.from === 'env'}
              placeholder={local ? 'http://localhost:8080/v1' : 'La oficial de Anthropic'}
              inputMode="url"
              spellCheck={false}
            />
          </Field>
          <Field label="Modelo" id="ajuste-modelo" env={settings.model}>
            <Input
              id="ajuste-modelo"
              value={form.model}
              onChange={(event) => set({ model: event.target.value })}
              disabled={settings.model.from === 'env'}
              placeholder={local ? 'El nombre que da tu servidor, p. ej. qwen3' : 'claude-haiku-5-5 (por defecto)'}
              spellCheck={false}
            />
          </Field>
        </div>

        {local && (
          <div className="grid gap-6 md:grid-cols-2">
            <Field label="Contexto (tokens)" id="ajuste-contexto">
              <Input
                id="ajuste-contexto"
                value={form.context_tokens}
                onChange={(event) => set({ context_tokens: event.target.value.replace(/\D/g, '') })}
                inputMode="numeric"
              />
              <p className="text-xs text-muted-foreground">El del servidor (--ctx-size en llama.cpp). Lo que no cabe se envía por partes.</p>
            </Field>
            <Field label="Razonamiento (thinking)">
              <ToggleGroup
                type="single"
                variant="outline"
                value={form.thinking}
                onValueChange={(value) => value && set({ thinking: value as Thinking })}
                aria-label="Razonamiento"
                className="flex-wrap justify-start"
              >
                <ToggleGroupItem value="server" className="px-3">
                  Lo decide el servidor
                </ToggleGroupItem>
                <ToggleGroupItem value="on" className="px-3">
                  Sí
                </ToggleGroupItem>
                <ToggleGroupItem value="off" className="px-3">
                  No
                </ToggleGroupItem>
              </ToggleGroup>
              <p className="text-xs text-muted-foreground">
                Pensar da mejores respuestas, pero tarda más. Puedes pedirlo en una pregunta concreta con «Pensar más».
              </p>
            </Field>
          </div>
        )}

        <div className="flex flex-wrap items-center gap-3 border-t pt-4">
          <Button onClick={() => save.mutate(pending)} disabled={!dirty || save.isPending}>
            {save.isPending && <Loader2 className="animate-spin" aria-hidden="true" />} Guardar
          </Button>
          <Button variant="outline" onClick={saveAndTest} disabled={save.isPending || test.isPending || Boolean(probe?.running)}>
            <PlugZap aria-hidden="true" /> {dirty ? 'Guardar y probar' : 'Probar conexión'}
          </Button>
          {dirty && (
            <Button variant="ghost" onClick={() => setForm(formOf(settings))}>
              Descartar cambios
            </Button>
          )}
          <ProbeResult probe={probe} />
        </div>
      </Panel>

      <LanguagePanel settings={settings} />

      <KeyPanel settings={settings} provider={form.provider} />
    </>
  )
}

function ProbeResult({ probe }: { probe: Settings['probe'] }) {
  if (!probe) return null
  if (probe.running)
    return (
      <span className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
        <Loader2 className="size-4 animate-spin" aria-hidden="true" /> Probando… si el servidor tiene que cargar el modelo, tarda un poco.
      </span>
    )
  const seconds = ((probe.millis ?? 0) / 1000).toLocaleString('es', { maximumFractionDigits: 1 })
  return probe.ok ? (
    <span className="flex items-center gap-2 text-sm text-accent-foreground" role="status">
      <CheckCircle2 className="size-4 text-brand" aria-hidden="true" /> Responde {probe.model?.replace(/^[^:]+:/, '')} en {seconds} s
    </span>
  ) : (
    <span className="flex min-w-0 basis-full gap-2 text-sm text-destructive" role="alert">
      <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> No responde: {probe.error}
    </span>
  )
}

const AUTO = 'auto'

/** El idioma se guarda al elegirlo: no depende del resto del formulario. */
function LanguagePanel({ settings }: { settings: Settings }) {
  const save = useSaveSettings()
  const { language } = settings
  return (
    <Panel className="flex flex-col gap-4">
      <h2 className="flex items-center gap-2 text-sm font-semibold">
        <Languages className="size-4 text-muted-foreground" aria-hidden="true" /> Idioma de las explicaciones
      </h2>
      <div className="flex flex-wrap items-center gap-3">
        <label htmlFor="ajuste-idioma" className="sr-only">
          Idioma de las explicaciones
        </label>
        <Select value={language.value ?? AUTO} onValueChange={(value) => save.mutate({ language: value })} disabled={save.isPending}>
          <SelectTrigger id="ajuste-idioma" className="w-72">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={AUTO}>Automático: el de la petición</SelectItem>
            {language.options.map((option) => (
              <SelectItem key={option.code} value={option.code}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {save.isPending && <Loader2 className="size-4 animate-spin text-muted-foreground" aria-hidden="true" />}
      </div>
      <p className="text-sm leading-relaxed text-muted-foreground">
        En el que el modelo escribe las guías, el guion, las respuestas a tus preguntas y las decisiones. Lo ya escrito se queda en su
        idioma: usa «Rehacer el guion» para volver a escribirlo. Al agente se lo pide al empezar cada sesión.
      </p>
    </Panel>
  )
}

function KeyPanel({ settings, provider }: { settings: Settings; provider: Provider }) {
  const saveKey = useSaveKey()
  const [key, setKey] = useState('')
  const { key: state } = settings
  const fromEnv = state.from === 'env'
  return (
    <Panel className="flex flex-col gap-4">
      <h2 className="flex items-center gap-2 text-sm font-semibold">
        <KeyRound className="size-4 text-muted-foreground" aria-hidden="true" /> Clave de la API
      </h2>
      <p className="text-sm text-muted-foreground">
        {fromEnv ? (
          <>
            Se usa la de la variable <Mono>{state.env}</Mono>, que manda sobre la guardada. Para cambiarla aquí, quítala de ahí.
          </>
        ) : state.from === 'file' ? (
          <>
            Guardada en <Mono>{state.file}</Mono>, que solo puedes leer tú. Escribe otra para sustituirla.
          </>
        ) : provider === 'openai-compatible' ? (
          'Sin clave. Un servidor en tu máquina no suele pedirla; uno remoto, sí.'
        ) : (
          'Sin clave: Claude la necesita.'
        )}
      </p>
      {state.problem && <Notice tone="warn">{state.problem}</Notice>}
      <form
        className="flex flex-wrap items-center gap-3"
        onSubmit={(event) => {
          event.preventDefault()
          if (key.trim()) saveKey.mutate(key, { onSuccess: () => setKey('') })
        }}
      >
        <label htmlFor="ajuste-clave" className="sr-only">
          Clave de la API
        </label>
        <Input
          id="ajuste-clave"
          type="password"
          autoComplete="off"
          value={key}
          onChange={(event) => setKey(event.target.value)}
          disabled={fromEnv}
          placeholder={state.from === 'file' ? 'Nueva clave' : 'Pega la clave'}
          className="max-w-md flex-[1_1_16rem]"
        />
        <Button type="submit" variant="outline" disabled={fromEnv || !key.trim() || saveKey.isPending}>
          Guardar clave
        </Button>
      </form>
      <p className="text-xs text-muted-foreground">No se guarda en config.toml ni se vuelve a mostrar.</p>
    </Panel>
  )
}

function Field({ label, id, env, children }: { label: string; id?: string; env?: { from: string; env?: string }; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-2">
      {id ? (
        <label htmlFor={id} className="text-[13px] font-semibold text-muted-foreground">
          {label}
        </label>
      ) : (
        <span className="text-[13px] font-semibold text-muted-foreground">{label}</span>
      )}
      {children}
      {env?.from === 'env' && (
        <p className="text-xs text-warn">
          Lo fija <Mono>{env.env}</Mono> (u opción de huella ui), que manda sobre lo guardado.
        </p>
      )}
    </div>
  )
}

function Notice({ tone, children }: { tone: 'warn'; children: ReactNode }) {
  return (
    <p className={tone === 'warn' ? 'flex gap-2 rounded-xl bg-warn-soft px-4 py-3 text-sm leading-relaxed text-warn' : ''} role="note">
      <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <span className="min-w-0">{children}</span>
    </p>
  )
}
