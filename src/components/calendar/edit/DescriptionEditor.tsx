// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// An event's description, formatted in PM (#884, C19 §7): the edit and create editors' Description.
// Google Calendar's seven buttons with their shortcuts (`FORMAT_BUTTONS`), paste that brings in text
// only, and a link panel, over a box the browser engine edits. React draws the toolbar and never owns
// the box's children: they are seeded from the codec's document, made into nodes one by one
// (descriptionDom.ts), and read back as a document after every change, then written as PM saves it
// (`serialiseDescription`), so whatever the engine made of the typing, a save writes something PM can
// read back and edit.
//
// Nothing here parses HTML or writes an HTML string into the page (INVARIANTS.md I-23): each command
// is a literal `execCommand` name, a paste or a drop from elsewhere brings in its plain text, a link's
// address goes only to the engine's link command and only after `linkFromTyped` took it (or, left as
// it was, `keptLink` kept the link's own), and the selection's words are read node by node, never as
// page text.
//
// Typing never seeds the box again, so the engine's own Undo survives; a new value that shows
// something else (Google's copy after a conflict) does. A link in the box never opens PM's own
// window on it, editable or not: a click on one does nothing, and Ctrl+click (⌘ on a Mac) opens a
// web or email address, as the formatted view does.
//
// `DescriptionView` shows what PM can of a description (a table, a colour as their words), where
// the editor can't or may not change it, with "Replace with plain text…" when the editor offers it.

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { openUrl } from "../../../lib/ipc";
import { IS_MAC } from "../../../lib/setupGuide";
import {
  docsEqual,
  isEmptyDoc,
  linkTarget,
  parseDescription,
  sameDescription,
  serialiseDescription,
  type DescDoc,
} from "../../../lib/calendarEdit/descriptionHtml";
import { docToNodes, nodesToDoc, rangeWords } from "../../../lib/calendarEdit/descriptionDom";
import {
  FORMAT_BUTTONS,
  formatKey,
  type FormatKeyName,
} from "../../../lib/calendarEdit/formatKeys";
import { useTheme } from "../../../theme";
import {
  BulletIcon,
  Button,
  ClearFormatIcon,
  ConfirmDialog,
  IconButton,
  LinkIcon,
  NumberIcon,
  Tooltip,
  cn,
} from "../../ui";
import { RichDescription } from "../parts/RichDescription";
import { LinkPanel } from "./LinkPanel";

export interface DescriptionEditorProps {
  /** The description as the draft holds it. */
  value: string;
  /** What the box holds now, written as PM saves it: called only when what it shows changed. */
  onChange: (value: string) => void;
  /** While saving: the box and the buttons stand still. */
  disabled: boolean;
  /** The id of the "Description" text naming the box. */
  labelledBy: string;
  /** The box's own id, so the "Description" text can focus it. */
  id?: string;
  /** Focus the box, the caret at the end (after "Replace with plain text…"). */
  autoFocus?: boolean;
  /** Opens a mail link's address in the mail app (`useMailLink`'s `open`), for Ctrl+click. */
  onMail: (target: string) => void;
}

/** The pictograms, in `FORMAT_BUTTONS`' keys: B, I and U as styled letters, as the Pinboard's. */
const ICONS: Record<FormatKeyName, ReactNode> = {
  bold: <span className="text-[0.6875rem] font-bold leading-none">B</span>,
  italic: <span className="font-serif text-[0.6875rem] italic leading-none">I</span>,
  underline: <span className="text-[0.6875rem] leading-none underline">U</span>,
  bulleted: <BulletIcon />,
  numbered: <NumberIcon />,
  link: <LinkIcon />,
  clear: <ClearFormatIcon />,
};

/** The box's own bookkeeping, outside React's state: the listeners read it as it is now. */
interface Box {
  /** The value the box last reported, or was seeded with: a value that shows the same is no reason
   *  to seed it again. */
  emitted: string | null;
  /** What the box shows, as last seeded or reported. */
  shown: DescDoc;
  /** The last selection seen inside the box, for a button reached by Tab. */
  kept: Range | null;
  /** A drag began inside the box: its drop is the engine's move, not text from elsewhere. */
  dragging: boolean;
  onChange: (value: string) => void;
}

/** What the box holds, reported when it shows something other than it did (critique C1-M2): an
 *  untouched box, or one that shows nothing either way, reports nothing. */
function report(box: HTMLElement, state: Box): void {
  const doc = nodesToDoc(box);
  if (docsEqual(doc, state.shown) || (isEmptyDoc(doc) && isEmptyDoc(state.shown))) return;
  state.shown = doc;
  const next = serialiseDescription(doc);
  state.emitted = next;
  state.onChange(next);
}

/** Whether the selection is inside the box. */
function selectionIn(box: HTMLElement): Selection | null {
  const sel = box.ownerDocument.getSelection();
  if (!sel || sel.rangeCount === 0) return null;
  return box.contains(sel.getRangeAt(0).commonAncestorContainer) ? sel : null;
}

/** Put the selection on `range` (or the end of the box), focused. */
function restore(box: HTMLElement, range: Range | null): void {
  box.focus();
  const sel = box.ownerDocument.getSelection();
  if (!sel) return;
  let at = range;
  if (at === null || !box.contains(at.commonAncestorContainer)) {
    at = box.ownerDocument.createRange();
    at.selectNodeContents(box);
    at.collapse(false);
  }
  sel.removeAllRanges();
  sel.addRange(at);
}

/** Text in at the selection, as plain words: the engine's own insert, so its Undo takes it back, or,
 *  in an engine without it, a text node in place of the selection. Line endings are line feeds
 *  first: WebKit's insert splits lines at a line feed only, and a carriage return left in a line
 *  would save as a break of its own (a CRLF file's lines, double-spaced). */
function insertPlain(box: HTMLElement, raw: string): void {
  const text = raw.replace(/\r\n?/g, "\n");
  if (text === "") return;
  const doc = box.ownerDocument;
  if (doc.execCommand?.("insertText", false, text)) return;
  const sel = selectionIn(box);
  if (!sel) return;
  const range = sel.getRangeAt(0);
  range.deleteContents();
  const words = doc.createTextNode(text);
  range.insertNode(words);
  range.setStartAfter(words);
  range.collapse(true);
  sel.removeAllRanges();
  sel.addRange(range);
}

/** The link around `node` inside the box, if any. */
function linkAround(node: Node | null, box: HTMLElement): HTMLAnchorElement | null {
  const el = node?.nodeType === 1 ? (node as Element) : (node?.parentElement ?? null);
  const link = el?.closest("a") ?? null;
  return link !== null && box.contains(link) ? link : null;
}

/** One formatting command on the selection, by its literal name (CodeQL reads the name as code): true
 *  when the engine ran it. Link opens its panel instead. */
function command(doc: Document, key: FormatKeyName): boolean {
  switch (key) {
    case "bold":
      return doc.execCommand?.("bold") ?? false;
    case "italic":
      return doc.execCommand?.("italic") ?? false;
    case "underline":
      return doc.execCommand?.("underline") ?? false;
    case "bulleted":
      return doc.execCommand?.("insertUnorderedList") ?? false;
    case "numbered":
      return doc.execCommand?.("insertOrderedList") ?? false;
    case "clear":
      return doc.execCommand?.("removeFormat") ?? false;
    case "link":
      return false;
  }
}

/** Whether the selection has a command's formatting: false when the engine can't say. */
function stateOf(read: () => boolean): boolean {
  try {
    return read();
  } catch {
    return false;
  }
}

type Pressed = Readonly<Record<FormatKeyName, boolean>>;
const NONE_PRESSED: Pressed = {
  bold: false,
  italic: false,
  underline: false,
  bulleted: false,
  numbered: false,
  link: false,
  clear: false,
};

/** What the toolbar shows pressed where the selection is now. */
function pressedAt(box: HTMLElement, sel: Selection): Pressed {
  const doc = box.ownerDocument;
  return {
    bold: stateOf(() => doc.queryCommandState("bold")),
    italic: stateOf(() => doc.queryCommandState("italic")),
    underline: stateOf(() => doc.queryCommandState("underline")),
    bulleted: stateOf(() => doc.queryCommandState("insertUnorderedList")),
    numbered: stateOf(() => doc.queryCommandState("insertOrderedList")),
    link: linkAround(sel.getRangeAt(0).commonAncestorContainer, box) !== null,
    clear: false,
  };
}

const samePressed = (a: Pressed, b: Pressed) =>
  FORMAT_BUTTONS.every(({ key }) => a[key] === b[key]);

const NO_BREAK_SPACE = String.fromCodePoint(0xa0);

/** Where `words` start, when the text node `node` holds them just before `offset` (a no-break space
 *  the engine typed reads as a space); else null. The Text field holds one line, so the words are one
 *  run of text, and the count is the engine's own, in code units, never graphemes. */
function typedBefore(node: Node, offset: number, words: string): number | null {
  if (node.nodeType !== 3 || offset < words.length) return null;
  const from = offset - words.length;
  const spaced = (s: string) => s.split(NO_BREAK_SPACE).join(" ");
  return spaced((node as Text).data.slice(from, offset)) === spaced(words) ? from : null;
}

/** A mail link's words when the link panel is given none: the address alone, as Google shows it. */
const addressWords = (url: string) => (/^mailto:/i.test(url) ? url.slice("mailto:".length) : url);

export function DescriptionEditor({
  value,
  onChange,
  disabled,
  labelledBy,
  id,
  autoFocus,
  onMail,
}: DescriptionEditorProps) {
  const { system } = useTheme();
  const host = useRef<HTMLDivElement>(null);
  const state = useRef<Box>({ emitted: null, shown: [], kept: null, dragging: false, onChange });
  const [pressed, setPressed] = useState<Pressed>(NONE_PRESSED);
  // The link panel, while open, and the range it is about (the selection, or the whole link).
  const [panel, setPanel] = useState<{
    text: string;
    href: string;
    editing: boolean;
    key: number;
  } | null>(null);
  const panelRange = useRef<Range | null>(null);
  // The toolbar is one Tab stop: the button last moved to (critique C2-M4).
  const [stop, setStop] = useState(0);
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);

  useLayoutEffect(() => {
    state.current.onChange = onChange;
  });

  // Seeded, unless the value shows what the box last reported: typing never seeds it again.
  useLayoutEffect(() => {
    const box = host.current;
    if (!box) return;
    const s = state.current;
    if (s.emitted !== null && sameDescription(value, s.emitted)) return;
    const doc = parseDescription(value).doc;
    // One by one: spread as arguments, a huge description's nodes would pass the engine's limit.
    box.replaceChildren();
    for (const node of docToNodes(doc, box.ownerDocument)) box.append(node);
    s.emitted = value;
    s.shown = doc;
  }, [value]);

  useEffect(() => {
    if (!autoFocus || !host.current) return;
    restore(host.current, null);
  }, [autoFocus]);

  // Saving: the panel closes with nothing applied.
  useEffect(() => {
    if (disabled) setPanel(null);
  }, [disabled]);

  useEffect(() => {
    const box = host.current;
    if (!box) return;
    const doc = box.ownerDocument;
    const s = state.current;
    // Where the selection is in the box: kept for a button reached by Tab, and what shows pressed.
    const onSelection = () => {
      const sel = selectionIn(box);
      if (!sel) return;
      s.kept = sel.getRangeAt(0).cloneRange();
      const now = pressedAt(box, sel);
      setPressed((was) => (samePressed(was, now) ? was : now));
    };
    // The backstop: a paste that skipped the paste event (a middle-click on Linux) is words too, and
    // a drop from elsewhere is the drop handler's alone. The data is read now, before the microtask.
    const onBeforeInput = (e: InputEvent) => {
      if (e.inputType === "insertFromDrop") {
        if (!s.dragging) e.preventDefault();
        return;
      }
      if (
        e.inputType === "insertFromPaste" ||
        e.inputType === "insertFromPasteAsQuotation" ||
        e.inputType === "insertFromYank"
      ) {
        e.preventDefault();
        const text = e.dataTransfer?.getData("text/plain") ?? "";
        queueMicrotask(() => {
          insertPlain(box, text);
          report(box, s);
        });
      }
    };
    doc.addEventListener("selectionchange", onSelection);
    box.addEventListener("beforeinput", onBeforeInput);
    return () => {
      doc.removeEventListener("selectionchange", onSelection);
      box.removeEventListener("beforeinput", onBeforeInput);
    };
  }, []);

  /** A button or its shortcut. Link's button closes its panel when it is open, as its
   *  `aria-expanded` says; its shortcut, pressed in the box, opens it again on the selection there. */
  function run(key: FormatKeyName, button = false) {
    const box = host.current;
    if (!box || disabled) return;
    if (key === "link" && button && panel !== null) {
      closePanel(box);
      return;
    }
    // A button reached by Tab: back to the box, where the selection last was.
    if (!selectionIn(box)) restore(box, state.current.kept);
    if (key === "link") {
      openPanel(box);
      return;
    }
    if (!command(box.ownerDocument, key)) return;
    report(box, state.current);
    const sel = selectionIn(box);
    if (sel) setPressed(pressedAt(box, sel));
  }

  function openPanel(box: HTMLElement) {
    const sel = selectionIn(box);
    if (!sel) return;
    let range = sel.getRangeAt(0).cloneRange();
    // In a link: the whole link, its words and address to change, or to remove.
    const link = linkAround(range.commonAncestorContainer, box);
    if (link !== null) {
      range = box.ownerDocument.createRange();
      range.selectNodeContents(link);
    }
    panelRange.current = range;
    // A new key each time: a panel opened again on another selection reads its fields afresh,
    // rather than keeping the words it was first given for the range it now acts on.
    setPanel((was) => ({
      text: rangeWords(range),
      href: link?.getAttribute("href") ?? "",
      editing: link !== null,
      key: (was?.key ?? 0) + 1,
    }));
  }

  /** The panel closes, however it closes: focus back in the box, on the range it was about. */
  function closePanel(box: HTMLElement) {
    setPanel(null);
    restore(box, panelRange.current);
  }

  function applyLink(text: string, url: string) {
    const box = host.current;
    const range = panelRange.current;
    if (!box || !range) return;
    closePanel(box);
    const doc = box.ownerDocument;
    const sel = doc.getSelection();
    // Linked at a caret: the caret comes back after the link, so the next key doesn't type over it
    // (critique C2-M5: the range it was about, which for a caret is a caret).
    const caret = range.collapsed;
    const words = text !== "" ? text : caret ? addressWords(url) : "";
    if (sel && words !== "" && words !== rangeWords(range)) {
      // New words, then the selection over them for the link (critique C1-M7: no graphemes
      // counted): back from where the engine left the caret, in the text it typed them into, else
      // from where they start. Where they start can move: both engines rewrite a typed no-break
      // space before the words, which pulls a kept range back over it.
      const start = range.cloneRange();
      start.collapse(true);
      if (doc.execCommand?.("insertText", false, words) && sel.focusNode !== null) {
        const typed = typedBefore(sel.focusNode, sel.focusOffset, words);
        const from: [Node, number] =
          typed !== null ? [sel.focusNode, typed] : [start.startContainer, start.startOffset];
        sel.setBaseAndExtent(from[0], from[1], sel.focusNode, sel.focusOffset);
      }
    }
    doc.execCommand?.("createLink", false, url);
    if (caret && sel && sel.rangeCount > 0) sel.collapseToEnd();
    report(box, state.current);
  }

  function removeLink() {
    const box = host.current;
    if (!box) return;
    closePanel(box);
    box.ownerDocument.execCommand?.("unlink");
    report(box, state.current);
  }

  /** A link in the box never navigates PM's window, editable or not; Ctrl+click (⌘ on a Mac) opens a
   *  web or email address the link rule takes (critique C2-H3). */
  function onLinkClick(e: ReactMouseEvent<HTMLDivElement>) {
    const link = linkAround(e.target as Node, e.currentTarget);
    if (link === null) return;
    e.preventDefault();
    if (e.type !== "click" || !(IS_MAC ? e.metaKey : e.ctrlKey)) return;
    const address = linkTarget(link.getAttribute("href") ?? "");
    if (address === null) return;
    if (/^mailto:/i.test(address)) onMail(address);
    else void openUrl(address).catch(() => {});
  }

  /** ←, →, Home and End move along the toolbar, its one Tab stop. */
  function onToolbarKey(e: ReactKeyboardEvent<HTMLDivElement>) {
    const at = buttons.current.findIndex((b) => b === e.target);
    if (at < 0) return;
    const n = FORMAT_BUTTONS.length;
    const to =
      e.key === "ArrowRight"
        ? (at + 1) % n
        : e.key === "ArrowLeft"
          ? (at + n - 1) % n
          : e.key === "Home"
            ? 0
            : e.key === "End"
              ? n - 1
              : null;
    if (to === null) return;
    e.preventDefault();
    setStop(to);
    buttons.current[to]?.focus();
  }

  return (
    <div className="flex flex-col gap-1.5">
      <div
        role="toolbar"
        aria-label="Description formatting"
        data-help="calendar-description-format"
        className="flex flex-wrap items-center gap-0.5"
        onKeyDown={onToolbarKey}
      >
        {FORMAT_BUTTONS.map((b, k) => {
          const on = b.key === "link" ? pressed.link || panel !== null : pressed[b.key];
          return (
            <Tooltip key={b.key} label={`${b.label} · ${b.hint}`}>
              <IconButton
                ref={(el) => {
                  buttons.current[k] = el;
                }}
                label={b.label}
                // The tooltip says it; a native one would show beside it.
                title=""
                variant={on ? "pressed" : "ghost"}
                aria-pressed={b.toggle ? on : undefined}
                aria-expanded={b.key === "link" ? panel !== null : undefined}
                aria-keyshortcuts={b.keys}
                tabIndex={k === stop ? 0 : -1}
                disabled={disabled}
                // The selection stays in the box, so the command lands where the caret is.
                onMouseDown={(e) => e.preventDefault()}
                onFocus={() => setStop(k)}
                onClick={() => run(b.key, true)}
              >
                {ICONS[b.key]}
              </IconButton>
            </Tooltip>
          );
        })}
      </div>
      {panel && (
        <LinkPanel
          key={panel.key}
          text={panel.text}
          href={panel.href}
          editing={panel.editing}
          onApply={applyLink}
          onRemove={removeLink}
          onCancel={() => host.current && closePanel(host.current)}
        />
      )}
      <div
        ref={host}
        id={id}
        role="textbox"
        aria-multiline="true"
        aria-labelledby={labelledBy}
        aria-disabled={disabled || undefined}
        tabIndex={disabled ? -1 : 0}
        contentEditable={!disabled}
        spellCheck
        className={cn(
          // Textarea's look, disabled as aria-disabled, and `pm-desc-box` for the lists and links
          // inside it (index.css). No `white-space` of its own: typed runs of spaces are kept as
          // no-break spaces, as Google's editor keeps them.
          "pm-desc-box max-h-[16rem] min-h-[6rem] w-full overflow-y-auto rounded-[var(--radius-sm)] border border-border2 bg-surface px-3 py-2 text-sm text-ink2 outline-none transition focus:border-accent aria-disabled:cursor-not-allowed aria-disabled:bg-transparent aria-disabled:text-ink3",
          system === "terminal" && "font-mono caret-[var(--accent-text)]",
        )}
        onFocus={(e) => {
          // Bold, italic and underline as their own elements, not styled spans.
          e.currentTarget.ownerDocument.execCommand?.("styleWithCSS", false, "false");
        }}
        onInput={(e) => report(e.currentTarget, state.current)}
        onBlur={(e) => report(e.currentTarget, state.current)}
        onKeyDown={(e) => {
          // Formatting keys on every platform: WebKitGTK has no Ctrl+U of its own, and Ctrl+K
          // stays off the command palette. Tab is left alone: it leaves the box.
          const key = formatKey(e);
          if (key === null) return;
          e.preventDefault();
          e.stopPropagation();
          run(key);
        }}
        onPaste={(e) => {
          e.preventDefault();
          if (disabled) return;
          insertPlain(e.currentTarget, e.clipboardData.getData("text/plain"));
          report(e.currentTarget, state.current);
        }}
        onDragStart={() => {
          state.current.dragging = true;
        }}
        onDragEnd={() => {
          state.current.dragging = false;
        }}
        onDrop={(e) => {
          // A drag inside the box moves as the engine moves it; text from elsewhere comes in as
          // words, where it was let go (critique C2-L9).
          if (state.current.dragging) return;
          e.preventDefault();
          if (disabled) return;
          const box = e.currentTarget;
          const doc = box.ownerDocument;
          const text = e.dataTransfer.getData("text/plain");
          const at =
            typeof doc.caretRangeFromPoint === "function"
              ? doc.caretRangeFromPoint(e.clientX, e.clientY)
              : null;
          restore(box, at ?? state.current.kept);
          insertPlain(box, text);
          report(box, state.current);
        }}
        onClick={onLinkClick}
        onAuxClick={onLinkClick}
      />
    </div>
  );
}

/** A description where the editor can't or may not change it, showing what PM can of it: Google's
 *  copy as the editor last read it, never the draft. "Replace with plain text…" when `onReplace` is
 *  given. */
export function DescriptionView({
  doc,
  labelledBy,
  describedBy,
  onReplace,
  onMail,
}: {
  doc: DescDoc;
  labelledBy: string;
  /** The line that says why it can't be changed, when there is one. */
  describedBy?: string;
  onReplace?: () => void;
  onMail: (target: string) => void;
}) {
  return (
    <div className="flex flex-col items-start gap-1.5">
      {/* A div: the view holds paragraphs and lists (critique C1-L3). */}
      <div
        role="group"
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        className="w-full rounded-[var(--radius-sm)] border border-border2 px-3 py-2 text-sm text-ink3"
      >
        <RichDescription doc={doc} onMail={onMail} />
      </div>
      {onReplace && (
        // Secondary: the editor's Save is the one primary button on screen.
        <Button variant="secondary" size="xs" onClick={onReplace}>
          Replace with plain text…
        </Button>
      )}
    </div>
  );
}

/** "Replace with plain text…" asked: nested inside the editor's dialog, so Escape closes only it.
 *  Google never asks this (it edits any description); PM keeps formatting it can't write back unless
 *  the user chooses to let it go (plan R9). `copy`: it's about a Duplicate, where no event exists yet
 *  and the original is never touched, so nothing goes from Google Calendar. */
export function ReplaceQuestion({
  open,
  copy = false,
  onReplace,
  onKeep,
}: {
  open: boolean;
  copy?: boolean;
  onReplace: () => void;
  onKeep: () => void;
}) {
  return (
    <ConfirmDialog
      open={open}
      title="Replace the description with plain text?"
      confirmLabel="Replace"
      cancelLabel="Keep the formatting"
      onConfirm={onReplace}
      onClose={onKeep}
    >
      PM keeps the words, line breaks and lists, and writes each link's address after its words.{" "}
      {copy
        ? "The original keeps its formatting; this copy won't have it."
        : "The formatting goes from Google Calendar when you save; until then, Cancel or close the editor to keep it."}
    </ConfirmDialog>
  );
}
