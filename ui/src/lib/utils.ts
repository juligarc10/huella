export { cn } from "cn"

/** Suelta el foco: tras elegir en un grupo de botones, las flechas vuelven a ser de la página. */
export function releaseFocus() {
  if (document.activeElement instanceof HTMLElement) document.activeElement.blur()
}
