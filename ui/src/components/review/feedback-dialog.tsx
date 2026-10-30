import { Check, ClipboardCopy, Loader2, Send } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'

import { Mono } from '@/components/bits'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import { useFeedback, useSetReviewStatus } from '@/lib/queries'

/** «Pedir cambios»: marca la sesión y da las instrucciones para el agente. */
export function FeedbackDialog({ session, openComments }: { session: string; openComments: number }) {
  const [open, setOpen] = useState(false)
  const [copied, setCopied] = useState(false)
  const feedback = useFeedback(session, open)
  const status = useSetReviewStatus(session)

  const copy = async () => {
    if (!feedback.data) return
    try {
      await navigator.clipboard.writeText(feedback.data.markdown)
      setCopied(true)
      toast.success('Instrucciones copiadas: pégalas en el chat del agente')
    } catch {
      toast.error('No se pudo copiar: selecciona el texto y cópialo a mano')
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next)
        setCopied(false)
        if (next) status.mutate('changes_requested')
      }}
    >
      <DialogTrigger asChild>
        <Button size="lg" className="min-h-11 bg-warn-strong text-white hover:bg-warn-strong/90">
          <Send aria-hidden="true" /> Pedir cambios{openComments > 0 && ` · ${openComments}`}
        </Button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>Instrucciones para el agente</DialogTitle>
          <DialogDescription>
            Pégalas en el chat del agente, o pídele que use la herramienta MCP <Mono>huella_review_feedback</Mono>. Cuando termine,
            aquí verás qué cambió después de tus comentarios.
          </DialogDescription>
        </DialogHeader>
        {feedback.isPending ? (
          <p className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="size-4 animate-spin" aria-hidden="true" /> Preparando…
          </p>
        ) : (
          <pre className="max-h-[55svh] overflow-auto rounded-xl bg-muted p-4 font-mono text-xs leading-relaxed whitespace-pre-wrap">
            {feedback.data?.markdown ?? feedback.error?.message}
          </pre>
        )}
        <DialogFooter>
          <Button onClick={copy} disabled={!feedback.data || feedback.data.items === 0}>
            {copied ? <Check aria-hidden="true" /> : <ClipboardCopy aria-hidden="true" />}
            {copied ? 'Copiadas' : 'Copiar instrucciones'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
