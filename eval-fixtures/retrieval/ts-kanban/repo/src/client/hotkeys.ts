/**
 * Global keyboard bindings for the board view.
 *
 *   n         new card in focused column
 *   e         edit focused card
 *   j / k     focus next / previous card
 *   h / l     move focused card to previous / next column
 *   /         focus the search box
 *   mod+z     undo, mod+shift+z redo
 *   ?         show the shortcut sheet
 */

export type CommandName =
  | "card.new"
  | "card.edit"
  | "focus.next"
  | "focus.prev"
  | "card.shiftLeft"
  | "card.shiftRight"
  | "search.focus"
  | "undo"
  | "redo"
  | "help";

const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

const BINDINGS: Record<string, CommandName> = {
  n: "card.new",
  e: "card.edit",
  j: "focus.next",
  k: "focus.prev",
  h: "card.shiftLeft",
  l: "card.shiftRight",
  "/": "search.focus",
  "mod+z": "undo",
  "mod+shift+z": "redo",
  "shift+?": "help",
};

export function chordOf(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (isMac ? e.metaKey : e.ctrlKey) parts.push("mod");
  if (e.altKey) parts.push("alt");
  if (e.shiftKey) parts.push("shift");
  parts.push(e.key.toLowerCase());
  return parts.join("+");
}

/** Elements where single-letter shortcuts must not fire. */
function isTypingTarget(el: EventTarget | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  return el.tagName === "INPUT" || el.tagName === "SELECT";
}

export function installShortcuts(
  target: Window,
  run: (cmd: CommandName) => void,
  overrides: Partial<Record<string, CommandName>> = {},
): () => void {
  const table: Record<string, CommandName> = { ...BINDINGS, ...overrides } as Record<string, CommandName>;
  const handler = (e: KeyboardEvent) => {
    const chord = chordOf(e);
    const cmd = table[chord];
    if (!cmd) return;
    if (!chord.startsWith("mod+") && isTypingTarget(e.target)) return;
    e.preventDefault();
    run(cmd);
  };
  target.addEventListener("keydown", handler);
  return () => target.removeEventListener("keydown", handler);
}
