// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What PM says when a link didn't open: the line under a description when a mail link didn't open
// (C19b, critique C2-M2), the link notice's words for every other link, the notice itself (inside
// the topmost dialog, where Settings' links are), and that no caller of `openUrl` in the tree is
// silent.

import { act, cleanup, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { useRef, type MouseEvent as ReactMouseEvent } from "react";
import ts from "typescript";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  dismissLinkFailure,
  linkFailureText,
  MAIL_APP_FAILED,
  mailFailureText,
  openLink,
  showLinkFailure,
  useLinkFailure,
} from "./linkFailure";
import { openUrl } from "./ipc";
import { closeContextMenu, openContextMenu } from "./contextMenu";
import { LinkNotice } from "../components/LinkNotice";
import { ContextMenuHost, Modal } from "../components/ui";
import { stopLinkAuxClick, useMailLink } from "../components/calendar/parts/useMailLink";
import { useDialogLayer } from "./useDialogLayer";

vi.mock("./ipc", () => ({
  openUrl: vi.fn(async () => undefined),
}));

afterEach(() => {
  cleanup();
  dismissLinkFailure();
  vi.mocked(openUrl).mockReset();
});

describe("mailFailureText", () => {
  it("names the address when the mail app didn't open", () => {
    expect(mailFailureText(MAIL_APP_FAILED, "mailto:ann@example.com?subject=Hi#x")).toBe(
      "PM couldn't open your mail app. The address is ann@example.com.",
    );
    expect(mailFailureText(MAIL_APP_FAILED, "mailto:a@example.com,b@example.org#top")).toBe(
      "PM couldn't open your mail app. The address is a@example.com,b@example.org.",
    );
  });

  it("says the backend's own words for a link it refused", () => {
    expect(mailFailureText("That doesn't look like an email address.", "mailto:a%0Ab")).toBe(
      "That doesn't look like an email address.",
    );
  });

  // Anything but the backend's sentence (a call that never reached it) still gives the address.
  it("names the address for a failure with no words", () => {
    expect(mailFailureText(new Error("x"), "mailto:ann@example.com")).toBe(
      "PM couldn't open your mail app. The address is ann@example.com.",
    );
  });

  // reader.rs pins its side: `a_launch_that_failed_says_which_app`.
  it("matches the backend's sentence", () => {
    expect(MAIL_APP_FAILED).toBe("PM couldn't open your mail app.");
  });
});

describe("linkFailureText", () => {
  it("gives a mail link, in any case, the mail words with its address", () => {
    expect(linkFailureText(MAIL_APP_FAILED, "MAILTO:ann@example.com")).toBe(
      "PM couldn't open your mail app. The address is ann@example.com.",
    );
  });

  it("gives a web link the backend's own words, or a plain line", () => {
    expect(linkFailureText("Couldn't open the link (NotFound).", "https://example.com")).toBe(
      "Couldn't open the link (NotFound).",
    );
    expect(linkFailureText(new Error("ipc"), "https://example.com")).toBe(
      "PM couldn't open the link.",
    );
  });
});

describe("LinkNotice", () => {
  it("is an empty live region until a link fails, then says why until dismissed", () => {
    render(<LinkNotice />);
    const region = screen.getByRole("status");
    expect(region.textContent).toBe("");
    act(() => showLinkFailure("PM couldn't open your mail app. The address is ann@example.com."));
    // The same region, so a screen reader hears the new line; the words are plain text.
    expect(screen.getByRole("status")).toBe(region);
    expect(region.textContent).toContain("The address is ann@example.com.");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(region.textContent).toBe("");
  });

  it("says when a middle-click on a description's web link didn't open", async () => {
    vi.mocked(openUrl).mockImplementationOnce(() =>
      Promise.reject("Couldn't open the link (NotFound)."),
    );
    render(<LinkNotice />);
    const link = document.createElement("a");
    link.setAttribute("href", "https://example.com/agenda");
    link.setAttribute("target", "_blank");
    const e = {
      defaultPrevented: false,
      target: link,
      button: 1,
      preventDefault: vi.fn(),
    } as unknown as ReactMouseEvent;
    await act(async () => stopLinkAuxClick(e));
    expect(openUrl).toHaveBeenCalledWith("https://example.com/agenda");
    expect(screen.getByRole("status").textContent).toContain("Couldn't open the link (NotFound).");
  });

  it("shows the newest failure only, as text never markup", () => {
    render(<LinkNotice />);
    act(() => showLinkFailure("first"));
    act(() => showLinkFailure("<b>second</b>"));
    const region = screen.getByRole("status");
    expect(region.textContent).not.toContain("first");
    expect(region.textContent).toContain("<b>second</b>");
    expect(region.querySelector("b")).toBeNull();
  });
});

/** The one z-index utility on `el`, as a number. Exactly one: `cn` is a plain joiner, so a second
 *  would leave the stylesheet's order to pick the winner. */
function zIndex(el: Element): number {
  const zs = Array.from(el.classList).filter((c) => /^z-(\[|\d)/.test(c));
  expect(zs).toHaveLength(1);
  return Number(zs[0].replace(/^z-\[?(\d+)\]?$/, "$1"));
}

/** Settings, and a confirmation opened inside it, as Settings' own dialogs are (Modal doesn't
 *  portal). */
function Dialogs({ settings, confirm }: { settings: boolean; confirm: boolean }) {
  return (
    <>
      <LinkNotice />
      <Modal open={settings} onClose={() => {}} label="Settings">
        <button type="button">Reset</button>
        <Modal open={confirm} onClose={() => {}} label="Confirm">
          <button type="button">Yes</button>
        </Modal>
      </Modal>
    </>
  );
}

/** A non-modal dialog layer, as the calendar's quick create is: the page beside it stays live. */
function Layer() {
  const ref = useRef<HTMLDivElement>(null);
  useDialogLayer(true, ref);
  return <div ref={ref} role="dialog" aria-label="Quick create" />;
}

/** The name of the dialog the notice's live region sits in; null in the window. */
const noticeDialog = () =>
  screen.getByRole("status").closest('[role="dialog"]')?.getAttribute("aria-label") ?? null;

// Settings is a dialog full of links. A modal dialog shuts the rest of the page out: under its
// scrim the notice was dimmed and a press on it closed Settings, its focus trap kept Tab from the
// notice's ×, and `aria-modal` leaves what is outside it out of the accessibility tree. So the
// notice lives in the topmost dialog while one is open. jsdom can't see paint order, so the first
// case compares the classes: in the window the notice and Modal's scrim are both `fixed` at the top
// level, so the higher number paints on top.
describe("LinkNotice with a dialog open", () => {
  it("sits on the rung over a dialog's scrim and under the right-click menu", () => {
    render(
      <>
        <LinkNotice />
        <Modal open onClose={() => {}} label="Settings">
          <p>body</p>
        </Modal>
        <ContextMenuHost />
      </>,
    );
    act(() =>
      openContextMenu({ clientX: 1, clientY: 1, preventDefault: () => {} }, "Menu", [
        { label: "One", onSelect: () => {} },
      ]),
    );
    const notice = zIndex(screen.getByRole("status"));
    expect(notice).toBe(60);
    expect(zIndex(screen.getByRole("dialog").parentElement!)).toBeLessThan(notice);
    expect(notice).toBeLessThan(zIndex(screen.getByRole("menu")));
    act(() => closeContextMenu());
  });

  it("can be dismissed, and the dialog stays open", () => {
    const onClose = vi.fn();
    render(
      <>
        <LinkNotice />
        <Modal open onClose={onClose} label="Settings">
          <p>body</p>
        </Modal>
      </>,
    );
    act(() => showLinkFailure("Couldn't open the link (NotFound)."));
    const dismiss = screen.getByRole("button", { name: "Dismiss" });
    fireEvent.mouseDown(dismiss);
    fireEvent.click(dismiss);
    expect(screen.getByRole("status").textContent).toBe("");
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeTruthy();
  });

  it("moves into the topmost dialog while one is open, and back once it closes", () => {
    const { rerender } = render(<Dialogs settings={false} confirm={false} />);
    act(() => showLinkFailure("PM couldn't open your mail app. The address is ann@example.com."));
    expect(noticeDialog()).toBeNull();
    rerender(<Dialogs settings confirm={false} />);
    expect(noticeDialog()).toBe("Settings");
    rerender(<Dialogs settings confirm />);
    expect(noticeDialog()).toBe("Confirm");
    rerender(<Dialogs settings confirm={false} />);
    expect(noticeDialog()).toBe("Settings");
    rerender(<Dialogs settings={false} confirm={false} />);
    expect(noticeDialog()).toBeNull();
    // One region all along, and the line came through every move.
    expect(screen.getAllByRole("status")).toHaveLength(1);
    expect(screen.getByRole("status").textContent).toContain("The address is ann@example.com.");
  });

  it("is in the dialog's Tab order, and its × hands focus back to the dialog", () => {
    const onClose = vi.fn();
    render(
      <>
        <LinkNotice />
        <Modal open onClose={onClose} label="Settings">
          <button type="button">First</button>
          <button type="button">Last</button>
        </Modal>
      </>,
    );
    act(() => showLinkFailure("Couldn't open the link (NotFound)."));
    const dialog = screen.getByRole("dialog", { name: "Settings" });
    const first = screen.getByRole("button", { name: "First" });
    const dismiss = screen.getByRole("button", { name: "Dismiss" });
    // Shift+Tab off the first control wraps to the trap's last stop, the notice's ×, and Tab off
    // that wraps back to the first: in the dialog's cycle, never out of it.
    first.focus();
    fireEvent.keyDown(first, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(dismiss);
    fireEvent.keyDown(dismiss, { key: "Tab" });
    expect(document.activeElement).toBe(first);
    // Pressed from the keyboard (Enter or Space clicks it): the notice goes, the dialog has focus
    // and stays open, and Shift+Tab from it stays inside.
    dismiss.focus();
    fireEvent.click(dismiss);
    expect(screen.getByRole("status").textContent).toBe("");
    expect(document.activeElement).toBe(dialog);
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(dialog, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Last" }));
  });

  // Each move remakes the ×, so the × a dialog was opened from is gone once it closes, and the trap
  // can't restore focus to it. The new × takes it back (useFocusTrap's hand-on).
  it("gets focus back when a dialog opened from its × closes, though the move remade it", () => {
    const { rerender } = render(<Dialogs settings={false} confirm={false} />);
    act(() => showLinkFailure("Couldn't open the link (NotFound)."));
    const before = screen.getByRole("button", { name: "Dismiss" });
    before.focus();
    rerender(<Dialogs settings confirm={false} />);
    expect(noticeDialog()).toBe("Settings");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Reset" }));
    rerender(<Dialogs settings={false} confirm={false} />);
    const after = screen.getByRole("button", { name: "Dismiss" });
    expect(after).not.toBe(before);
    expect(noticeDialog()).toBeNull();
    expect(document.activeElement).toBe(after);
  });

  it("gets focus back inside a dialog too, from a confirmation opened from its ×", () => {
    const { rerender } = render(<Dialogs settings confirm={false} />);
    act(() => showLinkFailure("Couldn't open the link (NotFound)."));
    screen.getByRole("button", { name: "Dismiss" }).focus();
    rerender(<Dialogs settings confirm />);
    expect(noticeDialog()).toBe("Confirm");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Yes" }));
    rerender(<Dialogs settings confirm={false} />);
    expect(noticeDialog()).toBe("Settings");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Dismiss" }));
  });

  it("gives nothing back once the notice was dismissed in the dialog, nor to a later one", () => {
    const { rerender } = render(<Dialogs settings={false} confirm={false} />);
    act(() => showLinkFailure("Couldn't open the link (NotFound)."));
    screen.getByRole("button", { name: "Dismiss" }).focus();
    rerender(<Dialogs settings confirm={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    rerender(<Dialogs settings={false} confirm={false} />);
    expect(screen.queryByRole("button", { name: "Dismiss" })).toBeNull();
    expect(document.activeElement).toBe(document.body);
    // A later notice, moved when a dialog opened from nothing closes, doesn't take focus either.
    act(() => showLinkFailure("Couldn't open the link (NotFound)."));
    rerender(<Dialogs settings confirm={false} />);
    rerender(<Dialogs settings={false} confirm={false} />);
    expect(document.activeElement).toBe(document.body);
  });

  it("stays out of a non-modal dialog, which leaves the page live", () => {
    const { rerender } = render(
      <>
        <LinkNotice />
        <Layer />
      </>,
    );
    expect(noticeDialog()).toBeNull();
    // Nor does one inside a modal dialog take it from that dialog.
    rerender(
      <>
        <LinkNotice />
        <Modal open onClose={() => {}} label="Settings">
          <Layer />
        </Modal>
      </>,
    );
    expect(noticeDialog()).toBe("Settings");
  });
});

describe("openLink", () => {
  it("takes the last link's notice down at once, then shows its own failure", async () => {
    const notice = renderHook(() => useLinkFailure());
    act(() => showLinkFailure("an earlier link's"));
    let fail: (e: unknown) => void = () => {};
    vi.mocked(openUrl).mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          fail = reject;
        }),
    );
    act(() => openLink("mailto:ann@example.com?subject=Hi"));
    expect(openUrl).toHaveBeenCalledExactlyOnceWith("mailto:ann@example.com?subject=Hi");
    expect(notice.result.current).toBeNull();
    await act(async () => fail(MAIL_APP_FAILED));
    expect(notice.result.current).toBe(
      "PM couldn't open your mail app. The address is ann@example.com.",
    );
  });

  it("says nothing for a link that opened", async () => {
    vi.mocked(openUrl).mockResolvedValueOnce(undefined);
    const notice = renderHook(() => useLinkFailure());
    await act(async () => openLink("https://example.com/"));
    expect(notice.result.current).toBeNull();
  });
});

describe("useMailLink", () => {
  // A description's mail link says its failure on its own line, so a notice an earlier link left
  // would be a second failure message, for another address.
  it("takes down an earlier link's notice, so two failures never show at once", async () => {
    vi.mocked(openUrl).mockRejectedValueOnce(MAIL_APP_FAILED);
    const notice = renderHook(() => useLinkFailure());
    const mail = renderHook(() => useMailLink("event-1"));
    act(() => showLinkFailure("PM couldn't open your mail app. The address is bob@example.com."));
    await act(async () => mail.result.current.open("mailto:ann@example.com"));
    expect(notice.result.current).toBeNull();
    expect(mail.result.current.failure).toBe(
      "PM couldn't open your mail app. The address is ann@example.com.",
    );
  });
});

// Every `openUrl` in the tree, read from the source as designGuards.test.ts reads it and parsed with
// the TypeScript compiler, already the repo's own. A click with no line of its own opens through
// `openLink`; the files below say a failure on their own line, and each takes the notice down
// first. A new caller fails here until it is one or the other.
const RAW = import.meta.glob("../**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
/** Repo-relative path → source, tests left out. Vite keys this folder's files `./`, the rest `../`. */
const SOURCES = new Map(
  Object.entries(RAW)
    .map(
      ([key, text]) => [key.replace(/^\.\//, "src/lib/").replace(/^\.\.\//, "src/"), text] as const,
    )
    .filter(([path]) => !/\.test\.[cm]?[jt]sx?$/.test(path)),
);

/** `text` parsed as the bundler reads the file at `path` (TSX for .tsx, JSX for .jsx, JS for .js,
 *  .mjs and .cjs, TS for the rest: `a < b > (c)` is a call only in TypeScript), each node knowing
 *  its parent. One the parser can't read throws: nothing is read off a tree it had to patch up. */
function parse(path: string, text: string): ts.SourceFile {
  const kind = path.endsWith(".tsx")
    ? ts.ScriptKind.TSX
    : path.endsWith(".jsx")
      ? ts.ScriptKind.JSX
      : /\.[cm]?js$/.test(path)
        ? ts.ScriptKind.JS
        : ts.ScriptKind.TS;
  const file = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, kind);
  // The parser's own errors: not in its public typings, but on every file it parses, and where
  // typescript-eslint reads them too. Gone after an upgrade, this throws rather than guess.
  const { parseDiagnostics: errors } = file as unknown as {
    parseDiagnostics?: readonly ts.Diagnostic[];
  };
  if (errors === undefined) throw new Error("this TypeScript keeps its parse errors elsewhere");
  if (errors.length > 0) {
    const why = ts.flattenDiagnosticMessageText(errors[0].messageText, " ");
    throw new Error(`${path} doesn't parse: ${why}`);
  }
  return file;
}

/** Every node in `file`, depth first. Comments, JSDoc among them, aren't nodes. */
function nodesOf(file: ts.SourceFile): ts.Node[] {
  const nodes: ts.Node[] = [];
  const visit = (node: ts.Node): void => {
    nodes.push(node);
    ts.forEachChild(node, visit);
  };
  visit(file);
  return nodes;
}

/** The line `node` starts on, from 0, and that line's text. */
function lineOf(file: ts.SourceFile, node: ts.Node): { line: number; text: string } {
  const { line } = file.getLineAndCharacterOfPosition(node.getStart(file));
  const starts = file.getLineStarts();
  return { line, text: file.text.slice(starts[line], starts[line + 1] ?? file.text.length).trim() };
}

/** The call `name` is the callee of, if it is one: `f(u)`, `f?.(u)` or `(f)(u)`. */
function callOf(name: ts.Identifier): ts.CallExpression | undefined {
  let callee: ts.Node = name;
  while (ts.isParenthesizedExpression(callee.parent)) callee = callee.parent;
  const call = callee.parent;
  return ts.isCallExpression(call) && call.expression === callee ? call : undefined;
}

/** Every identifier named `openUrl` in `file`, however it is spelled: the parser reads a Unicode
 *  escape in a name as the letter it stands for. */
const openUrlNames = (file: ts.SourceFile) =>
  nodesOf(file).filter((n): n is ts.Identifier => ts.isIdentifier(n) && n.text === "openUrl");

/** Whether `node` is a type. The parser counts `make()<string>` (an instantiation expression) as
 *  one, though it is `make()` with its type given, so that is read as the expression it is. */
const isType = (node: ts.Node) => ts.isTypeNode(node) && !ts.isExpressionWithTypeArguments(node);
const isDeclare = (modifier: ts.ModifierLike) => modifier.kind === ts.SyntaxKind.DeclareKeyword;

/** Whether `node` makes a call or a `new` of its own: one outside any function, class or type it
 *  defines, whose calls it only defines, and outside any `declare`, which the build erases as it
 *  does a type. A function's parameters are its own, so a default there is left out with its body,
 *  and a type (`{ m(): void }`) holds no call. */
function makesACall(node: ts.Node): boolean {
  // Read from the top down, so a `declare` (`declare enum E { A = f() }`) is met before its body.
  const declared = ts.canHaveModifiers(node) && ts.getModifiers(node)?.some(isDeclare) === true;
  if (declared) return false;
  if (ts.isCallExpression(node) || ts.isNewExpression(node)) return true;
  if (ts.isFunctionLike(node) || ts.isClassLike(node) || isType(node)) return false;
  if (ts.isInterfaceDeclaration(node) || ts.isTypeAliasDeclaration(node)) return false;
  return ts.forEachChild(node, makesACall) ?? false;
}

/** A rejection handler that does something: an inline arrow or function, not a generator (a call
 *  never runs its body), whose body makes a call of its own. So an arrow whose body is a function
 *  fails, and the handler's own parameters are left out: a default there runs only for a failure
 *  with no reason. What the call says is the reviewer's to read: this can't tell `setFailed(true)`
 *  from `console.log(e)`. */
function handlerSaysSomething(handler: ts.Expression): boolean {
  if (!ts.isArrowFunction(handler) && !ts.isFunctionExpression(handler)) return false;
  return handler.asteriskToken === undefined && makesACall(handler.body);
}

/** Kinds a statement between the dismiss and the call may be made of, types aside: a declaration
 *  of names, property reads and literals, with no call, `await` or function in it, as
 *  CalendarEventPopover's `const on = event.id;` is, or `const { id: on } = event;`. A read is by
 *  dot or bracket (`ids[i]`, `event?.["id"]`) and may have `?.`, `!`, `as T`, `satisfies T` or
 *  parentheses on it: the build erases all but `?.`, which only skips the read. A call, `await` or
 *  function under any of them, a bracket's key included, is still refused. */
const READS = new Set([
  ts.SyntaxKind.VariableStatement,
  ts.SyntaxKind.VariableDeclarationList,
  ts.SyntaxKind.VariableDeclaration,
  ts.SyntaxKind.ObjectBindingPattern,
  ts.SyntaxKind.BindingElement,
  ts.SyntaxKind.Identifier,
  ts.SyntaxKind.PropertyAccessExpression,
  ts.SyntaxKind.ElementAccessExpression,
  ts.SyntaxKind.QuestionDotToken,
  ts.SyntaxKind.NonNullExpression,
  ts.SyntaxKind.AsExpression,
  ts.SyntaxKind.SatisfiesExpression,
  ts.SyntaxKind.ParenthesizedExpression,
  ts.SyntaxKind.StringLiteral,
  ts.SyntaxKind.NumericLiteral,
  ts.SyntaxKind.NoSubstitutionTemplateLiteral,
  ts.SyntaxKind.TrueKeyword,
  ts.SyntaxKind.FalseKeyword,
  ts.SyntaxKind.NullKeyword,
]);
const onlyReads = (node: ts.Node): boolean =>
  isType(node) ||
  (READS.has(node.kind) &&
    ts.forEachChild(node, (child) => !onlyReads(child) || undefined) === undefined);

/** `dismissLinkFailure();`, as a statement of its own. */
function isDismiss(statement: ts.Statement): boolean {
  if (!ts.isExpressionStatement(statement) || !ts.isCallExpression(statement.expression)) {
    return false;
  }
  const { expression: callee, arguments: args, questionDotToken } = statement.expression;
  return (
    ts.isIdentifier(callee) &&
    callee.text === "dismissLinkFailure" &&
    args.length === 0 &&
    questionDotToken === undefined
  );
}

/** Whether `dismissLinkFailure();` is the statement straight before `statement`, in its block, with
 *  nothing between but declarations of reads. */
function dismissedBefore(statement: ts.Statement): boolean {
  const block = statement.parent;
  if (
    !ts.isBlock(block) &&
    !ts.isSourceFile(block) &&
    !ts.isModuleBlock(block) &&
    !ts.isCaseOrDefaultClause(block)
  ) {
    return false;
  }
  let i = block.statements.indexOf(statement) - 1;
  while (i >= 0 && onlyReads(block.statements[i])) i--;
  return i >= 0 && isDismiss(block.statements[i]);
}

/** What is wrong with each `openUrl` call in `file`, by line: empty when every one is a statement of
 *  its own (under `void`, `await` or parentheses) with `dismissLinkFailure();` straight before it,
 *  and handles its failure on the call itself, as `.catch(h)` or `.then(ok, h)` with `h` saying
 *  something. */
function openUrlProblems(file: ts.SourceFile): string[] {
  const problems: string[] = [];
  for (const name of openUrlNames(file)) {
    const call = callOf(name);
    if (call === undefined) continue;
    const say = (what: string) => problems.push(`line ${lineOf(file, call).line + 1}: ${what}`);
    // The chain on the call: each `.then(…)`, `.catch(…)` or `.finally(…)` made on the one before
    // it. A `.finally(f)` neither handles a failure nor swallows one, so it adds no handler.
    const handlers: ts.Expression[] = [];
    let chain: ts.Expression = call;
    for (;;) {
      const link = chain.parent;
      if (!ts.isPropertyAccessExpression(link) || link.expression !== chain) break;
      const method = link.name.text;
      const next = link.parent;
      if (link.questionDotToken || !["then", "catch", "finally"].includes(method)) break;
      if (!ts.isCallExpression(next) || next.expression !== link || next.questionDotToken) break;
      const at = method === "catch" ? 0 : 1;
      if (method !== "finally" && next.arguments.length > at) handlers.push(next.arguments[at]);
      chain = next;
    }
    let outer: ts.Node = chain;
    while (
      ts.isVoidExpression(outer.parent) ||
      ts.isAwaitExpression(outer.parent) ||
      ts.isParenthesizedExpression(outer.parent)
    ) {
      outer = outer.parent;
    }
    if (!ts.isExpressionStatement(outer.parent)) {
      say("not a statement of its own, so its dismiss and its handler can't be read");
      continue;
    }
    if (!dismissedBefore(outer.parent)) say("no dismissLinkFailure(); straight before it");
    if (handlers.length === 0) say("no rejection handler, .catch(h) or .then(ok, h)");
    for (const h of handlers.filter((h) => !handlerSaysSomething(h))) {
      say(`a rejection handler that says nothing: ${h.getText(file)}`);
    }
  }
  return problems;
}

// A call is only one way to reach `openUrl`: `ipc.openUrl(u)` after `import * as ipc`, an alias, or
// the function handed on as a value would each open a link the check above never sees. So every
// identifier named `openUrl` is accounted for. Comments aren't code, so they may name it freely.
//
// `ipc["openUrl"](u)` needs no rule of its own. A lookup needs lib/ipc as an object, and no file
// can hold one: a named import brings in names, never the module, every other way to the module is
// refused below, and ipc.ts putting `openUrl` in an object would be a mention it isn't allowed. A
// string, template, regex or JSX text that spells the name is refused all the same, so nothing
// can be looked up by it on anything.

/** The file name a module specifier ends in, %-escapes decoded, once its `.` and `..` are walked
 *  (`ipc/.` and `ipc/x/..` are ipc), without its folders, query, hash or extension, in lower case:
 *  `ipc` for every spelling of lib/ipc, and for any other module so named, which fails closed with
 *  it. One that ends in no name (`.`, `..`, `./`) is read as `ipc` too: it can't be named. */
function moduleName(spec: string): string {
  let decoded = spec;
  try {
    decoded = decodeURIComponent(spec);
  } catch {
    // Not an escape: as written.
  }
  const names: string[] = [];
  for (const part of decoded.replace(/[?#][\s\S]*$/, "").split(/[/\\]/)) {
    if (part === "" || part === ".") continue;
    if (part === ".." && names.length > 0 && names[names.length - 1] !== "..") names.pop();
    else names.push(part);
  }
  const last = names[names.length - 1];
  if (last === undefined || last === "..") return "ipc";
  return last.replace(/\.[^.]*$/, "").toLowerCase();
}

/** Whether `spec`, imported in the file at `from`, is src/lib's module `lib` (`ipc`,
 *  `linkFailure`): a relative path to it, with no extension, `.ts` or `.js`, in any case. */
function isLib(from: string, spec: string, lib: string): boolean {
  if (!/^\.\.?\//.test(spec)) return false;
  const parts = from.split("/").slice(0, -1);
  for (const part of spec.split("/")) {
    if (part === "..") {
      if (parts.pop() === undefined) return false;
    } else if (part !== ".") parts.push(part);
  }
  return new RegExp(`^src/lib/${lib}(?:\\.[jt]s)?$`, "i").test(parts.join("/"));
}

/** A specifier that names lib/ipc, some other module called `ipc`, or no module at all. */
const namesIpc = (spec: ts.Expression) =>
  ts.isStringLiteral(spec) && moduleName(spec.text) === "ipc";
/** A load's arguments that are one plain string naming a module other than lib/ipc. */
const plainOther = (args: readonly ts.Expression[]) =>
  args.length === 1 && ts.isStringLiteral(args[0]) && !namesIpc(args[0]);

/** How `node` takes lib/ipc other than by a named import, if it does: a namespace import, a
 *  re-export of all of it, a require or a dynamic import of it or of anything but a plain string,
 *  or Vite's `import.meta.glob`, whose patterns this doesn't read (refused anywhere). */
function reachesIpcWhole(node: ts.Node): string | undefined {
  if (ts.isImportDeclaration(node)) {
    const bindings = node.importClause?.namedBindings;
    if (bindings && ts.isNamespaceImport(bindings) && namesIpc(node.moduleSpecifier)) {
      return "imports lib/ipc whole, so its openUrl can't be followed";
    }
  } else if (ts.isExportDeclaration(node)) {
    const all = node.exportClause === undefined || ts.isNamespaceExport(node.exportClause);
    if (all && node.moduleSpecifier && namesIpc(node.moduleSpecifier)) {
      return "re-exports all of lib/ipc, so its openUrl can't be followed";
    }
  } else if (ts.isImportEqualsDeclaration(node)) {
    const ref = node.moduleReference;
    if (ts.isExternalModuleReference(ref) && !plainOther([ref.expression])) {
      return "requires lib/ipc, or a module this can't name";
    }
  } else if (ts.isCallExpression(node)) {
    const callee = node.expression;
    const loads =
      callee.kind === ts.SyntaxKind.ImportKeyword ||
      (ts.isIdentifier(callee) && callee.text === "require");
    if (loads && !plainOther(node.arguments)) return "loads lib/ipc, or a module this can't name";
  } else if (ts.isMetaProperty(node) && node.keywordToken === ts.SyntaxKind.ImportKeyword) {
    const use = node.parent;
    const glob = ts.isPropertyAccessExpression(use) && use.name.text.startsWith("glob");
    if (glob || ts.isElementAccessExpression(use)) {
      return "uses import.meta.glob, whose patterns this doesn't read";
    }
  }
  return undefined;
}

/** The literal kinds whose text could spell a name to look up. */
const SPELLINGS = new Set([
  ts.SyntaxKind.StringLiteral,
  ts.SyntaxKind.NoSubstitutionTemplateLiteral,
  ts.SyntaxKind.TemplateHead,
  ts.SyntaxKind.TemplateMiddle,
  ts.SyntaxKind.TemplateTail,
  ts.SyntaxKind.RegularExpressionLiteral,
  ts.SyntaxKind.JsxText,
]);
/** A literal's text, cooked and (for a template's parts) raw. */
function spelling(node: ts.Node): string {
  const literal = node as ts.LiteralLikeNode & { rawText?: string };
  return `${literal.text} ${literal.rawText ?? ""}`;
}

/** Whether `name`, in the file at `path`, is what a named import from src/lib's module `lib` brings
 *  in, under its own name. */
function isLibImport(path: string, name: ts.Identifier, lib: string): boolean {
  const specifier = name.parent;
  if (!ts.isImportSpecifier(specifier) || specifier.name !== name || specifier.propertyName) {
    return false;
  }
  const declaration = specifier.parent.parent.parent;
  return (
    ts.isImportDeclaration(declaration) &&
    ts.isStringLiteral(declaration.moduleSpecifier) &&
    isLib(path, declaration.moduleSpecifier.text, lib)
  );
}

/** What an `openUrl` that is neither a call nor an import is, for its pin: a top-level
 *  `export const`, a type member, a property read, a property's name, or `other`. */
function roleOf(name: ts.Identifier): string {
  const p = name.parent;
  if (ts.isVariableDeclaration(p) && p.name === name) {
    const statement = p.parent.parent;
    const isConst = (p.parent.flags & ts.NodeFlags.BlockScoped) === ts.NodeFlags.Const;
    const exported =
      ts.isVariableStatement(statement) &&
      ts.isSourceFile(statement.parent) &&
      statement.modifiers?.some((m) => m.kind === ts.SyntaxKind.ExportKeyword) === true;
    if (isConst && exported) return "export const";
  }
  if (ts.isPropertySignature(p) && p.name === name) return "type member";
  if (ts.isPropertyAccessExpression(p) && p.name === name) return "property access";
  if (ts.isPropertyAssignment(p) && p.name === name) return "property name";
  return "other";
}

/** Every `openUrl` in the file at `path`: its calls (counted), the names a named import from
 *  lib/ipc brings in (skipped), and every other one, by its role and its line, for the pins below;
 *  and every literal that spells the command, `open_url`, as `command` and its line, for the same
 *  pins, so a second wrapper of it shows. Plus what is wrong: an import under another name, the
 *  name spelled in a literal, or lib/ipc taken some other way. */
function openUrlMentions(path: string, file: ts.SourceFile) {
  let calls = 0;
  const others: string[] = [];
  const problems: string[] = [];
  const say = (node: ts.Node, what: string) =>
    problems.push(`line ${lineOf(file, node).line + 1}: ${what}`);
  for (const node of nodesOf(file)) {
    const whole = reachesIpcWhole(node);
    if (whole !== undefined) say(node, whole);
    if (SPELLINGS.has(node.kind) && spelling(node).includes("openUrl")) {
      say(node, "spells openUrl in a literal, so a lookup by it can't be followed");
    }
    if (SPELLINGS.has(node.kind) && spelling(node).includes("open_url")) {
      others.push(`command: ${lineOf(file, node).text}`);
    }
    if (ts.isImportSpecifier(node) && node.propertyName?.text === "openUrl") {
      say(node, "imports openUrl under another name");
    }
    if (!ts.isIdentifier(node) || node.text !== "openUrl") continue;
    if (callOf(node) !== undefined) calls++;
    else if (!isLibImport(path, node, "ipc")) {
      others.push(`${roleOf(node)}: ${lineOf(file, node).text}`);
    }
  }
  return { calls, others, problems };
}

/** What is wrong with `dismissLinkFailure` in the file at `path`, by line: empty when the name is
 *  bound once, to linkFailure.ts's own (its declaration there, or a named import of it from
 *  lib/linkFailure under its own name), and every other one is a call. So no stand-in of that name
 *  passes as taking the notice down. */
function dismissProblems(path: string, file: ts.SourceFile): string[] {
  const problems: string[] = [];
  let bound = 0;
  for (const node of nodesOf(file)) {
    if (!ts.isIdentifier(node) || node.text !== "dismissLinkFailure") continue;
    const p = node.parent;
    const declared =
      path === "src/lib/linkFailure.ts" &&
      ts.isFunctionDeclaration(p) &&
      p.name === node &&
      ts.isSourceFile(p.parent);
    if (declared || isLibImport(path, node, "linkFailure")) bound++;
    else if (callOf(node) === undefined) {
      problems.push(
        `line ${lineOf(file, node).line + 1}: a dismissLinkFailure not its own or called`,
      );
    }
  }
  if (bound !== 1) {
    problems.push(`dismissLinkFailure is bound to linkFailure.ts's ${bound} times, not once`);
  }
  return problems;
}

/** A snippet's call problems, read as a .tsx file is. */
const problemsIn = (text: string) => openUrlProblems(parse("src/components/X.tsx", text));

describe("the openUrl guard's reading", () => {
  it("passes a call that takes the notice down and says its failure, in either spelling", () => {
    for (const ok of [
      "const open = () => {\n  dismissLinkFailure();\n  void openUrl(u).catch((e: unknown) => setSaid(e));\n};",
      "setFailed(null);\ndismissLinkFailure();\nconst on = id;\nopenUrl(u).then(\n  () => setOk(true),\n  () => setFailed(on),\n);",
      "dismissLinkFailure();\nopenUrl(u).then(() => setOk(true)).catch(function () {\n  setFailed(true);\n});",
      // An object destructuring of reads is a declaration of reads too (round 13).
      "dismissLinkFailure();\nconst { id: on, kind } = event;\nopenUrl(u).catch(() => setFailed(on));",
      // So is a read with `?.`, `!`, `as`, `satisfies` or parentheses on it (round 14).
      "dismissLinkFailure();\nconst on = event?.id;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst on = event.id!;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst on = event.id as EventId;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst on = event.id satisfies string;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst on = (event as Ev).id;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst on = (event as Ev)?.id!;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst entry = conference.entryPoints[0];\nopenUrl(u).catch(() => setFailed(entry));",
      'dismissLinkFailure();\nconst on = event?.["id"];\nopenUrl(u).catch(() => setFailed(on));',
    ]) {
      expect(problemsIn(ok), ok).toEqual([]);
    }
  });

  it("steps over a .finally(f), which neither handles a failure nor swallows one (round 13)", () => {
    for (const ok of [
      "setBusy(true);\ndismissLinkFailure();\nvoid openUrl(u)\n  .catch(() => setFailed(true))\n  .finally(() => setBusy(false));",
      "dismissLinkFailure();\nopenUrl(u).finally(() => setBusy(false)).catch(() => setFailed(true));",
    ]) {
      expect(problemsIn(ok), ok).toEqual([]);
    }
    // A `.finally` with no handler handles nothing, and its second argument is never called.
    for (const bad of [
      "dismissLinkFailure();\nvoid openUrl(u).finally(() => setBusy(false));",
      "dismissLinkFailure();\nvoid openUrl(u).finally(() => setBusy(false), () => setFailed(true));",
    ]) {
      expect(problemsIn(bad), bad).toEqual([
        "line 2: no rejection handler, .catch(h) or .then(ok, h)",
      ]);
    }
    expect(
      problemsIn(
        "dismissLinkFailure();\nopenUrl(u).catch(() => {}).finally(() => setBusy(false));",
      ),
    ).toEqual(["line 2: a rejection handler that says nothing: () => {}"]);
  });

  it("names a missing, shared or distant dismiss, and a failure with no handler", () => {
    for (const bad of [
      "dismissLinkFailure();\nvoid openUrl(u).then(() => setOk(true), () => {});",
      "dismissLinkFailure();\nopenUrl(u).then(() => setOk(true));",
      "try {\n  dismissLinkFailure();\n  await openUrl(u);\n} catch {\n  setFailed(true);\n}",
      "openUrl(u).catch(() => setFailed(true));",
      "openUrl(u).catch(() => setFailed(true));\ndismissLinkFailure();",
      "dismissLinkFailure();\nopenUrl(a).catch(() => setA(true));\nopenUrl(b).catch(() => setB(true));",
      "const d = () => dismissLinkFailure();\nopenUrl(u).catch(() => setFailed(true));",
      "if (up) {\n  dismissLinkFailure();\n}\nopenUrl(u).catch(() => setFailed(true));",
      // Only declarations of reads may come between, and the call is the statement, not a
      // function that makes it later.
      "dismissLinkFailure();\nsetOther(true);\nopenUrl(u).catch(() => setFailed(true));",
      "dismissLinkFailure();\nconst on = await id;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst f = make()<string>;\nopenUrl(u).catch(() => setFailed(true));",
      "dismissLinkFailure();\nconst { id: on = make() } = event;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst { [key()]: on } = event;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst on = event?.make();\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst on = make()!;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst on = ids[next()];\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst on = (await x) as string;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nconst on = (make()).id;\nopenUrl(u).catch(() => setFailed(on));",
      "dismissLinkFailure();\nlater(() => openUrl(u).catch(() => setFailed(true)));",
    ]) {
      expect(problemsIn(bad), bad).not.toEqual([]);
    }
  });

  it("names a handler that says nothing, however it's spelled", () => {
    for (const handler of [
      "() => undefined",
      "(_e) => void 0",
      "() => { /* it's fine (really) */ }",
      "() => {\n  return;\n}",
      "function () {}",
      "noop",
      // Bodies that make no call, however they're spelled.
      "() => {\n  return undefined;\n}",
      "() => (undefined)",
      "(e) => void e",
      "() => void (0)",
      "(e) => e",
      "() => {\n  void e;\n}",
      '() => "setFailed(true)"',
      "() => () => setFailed(true)",
      "() => { if (e) {} }",
      // Bodies whose only call is in a function they define, not one they make.
      "() => {\n  const f = () => setFailed(true);\n}",
      "() => [() => setFailed(true)]",
      "() => ({ say() { setFailed(true); } })",
      "() => {\n  function f() {\n    setFailed(true);\n  }\n}",
      "() => {\n  const f = async (e): Promise<void> => say(e);\n}",
      // A method's or `function`'s body is one too, whatever return type comes before it.
      "() => ({ say(): void { setFailed(true); } })",
      "() => {\n  function f(): void {\n    setFailed(true);\n  }\n}",
      "() => ({ get x(): boolean { return setFailed(true); } })",
      "() => {\n  class A {\n    m(): void {\n      setFailed(true);\n    }\n  }\n}",
      "() => ({ m(): { ok: boolean } { return setFailed(true); } })",
      "() => ({ m(): Promise<{ ok: boolean }> { return setFailed(true); } })",
      "() => ({ m(): A | { ok: boolean } { return setFailed(true); } })",
      "() => ({ m(): () => void { return setFailed(true); } })",
      // A return type on the handler itself changes nothing.
      "(e): void => {}",
      "(e): (() => void) => () => setFailed(e)",
      "function (): void {}",
      // A type's method signature is no call (round 8): in a return type, an annotation, an
      // interface, an `as` and a type alias.
      "function (): { m(): void; n: number } {}",
      "() => {\n  let x: { m(): void };\n}",
      "() => {\n  interface I {\n    m(): void;\n  }\n}",
      "(e) => e as { m(): void }",
      "() => {\n  type T = { m(): void };\n}",
      // A default in a function the handler only defines is that function's (round 8).
      "() => {\n  function f(a = setFailed(true)) {}\n}",
      "() => {\n  const f = (a = setFailed(true)) => 0;\n}",
      "() => ({ m(a = setFailed(true)): void {} })",
      "() => {\n  const f = function (a = setFailed(true)) {};\n}",
      // A call only in a class's method, or only in a type, is none the handler makes.
      "() => class {\n  m() {\n    setFailed(true);\n  }\n}",
      "() => {\n  let x: { [setFailed(true)]: number };\n}",
      // The handler's own default runs only for a failure with no reason, and a generator's body
      // doesn't run when it is called.
      "(e = setFailed(true)) => {}",
      "function* () {\n  setFailed(true);\n}",
      // Code under `declare` is erased, as a type is, so a call there never runs (round 9).
      "() => {\n  // @ts-expect-error -- erased\n  declare enum E { A = setFailed(e) }\n}",
      "() => {\n  // @ts-expect-error -- erased\n  declare module M { const x = setFailed(e); }\n}",
    ]) {
      const text = `dismissLinkFailure();\nopenUrl(u).catch(${handler});`;
      expect(problemsIn(text), text).toEqual([
        `line 2: a rejection handler that says nothing: ${handler}`,
      ]);
    }
  });

  it("passes a call made through a void, an optional call, a member or a block", () => {
    for (const handler of [
      "() => void setFailed(true)",
      "(e) => onFailed?.(e)",
      "(e: unknown) => setSaid({ on, text: mailFailureText(e, target) })",
      "() => {\n  // say it\n  refs.current.fail(true);\n}",
      "() => {\n  if (on) {\n    setFailed(true);\n  }\n}",
      "() => {\n  const f = () => setFailed(true);\n  f();\n}",
      "() => (() => setFailed(true))()",
      "(e) => (on ? setFailed(e) : setOther(e))",
      "() => {\n  for await (const x of xs) {\n    setFailed(x);\n  }\n}",
      "() => {\n  function f(): void {\n    setFailed(true);\n  }\n  f();\n}",
      "(e: unknown): void => setFailed(e)",
      "async (e): Promise<void> => {\n  await say(e);\n}",
      "function (): void {\n  setFailed(true);\n}",
      "() => {\n  const f = make()<string>;\n}",
    ]) {
      const text = `dismissLinkFailure();\nopenUrl(u).catch(${handler});`;
      expect(problemsIn(text), text).toEqual([]);
    }
  });

  it("reads a JavaScript file as JavaScript, as the bundler does (round 9)", () => {
    // In TypeScript `say < e > (e)` is a generic call; in JavaScript it is two comparisons.
    const text = "dismissLinkFailure();\nopenUrl(u).catch((e) => say < e > (e));";
    expect(openUrlProblems(parse("src/components/X.ts", text))).toEqual([]);
    for (const path of ["X.js", "X.mjs", "X.cjs", "X.jsx"].map((n) => `src/components/${n}`)) {
      expect(openUrlProblems(parse(path, text)), path).toEqual([
        "line 2: a rejection handler that says nothing: (e) => say < e > (e)",
      ]);
    }
  });

  it("knows dismissLinkFailure only as linkFailure.ts's own (round 9)", () => {
    const read = (text: string, path = "src/components/X.tsx") =>
      dismissProblems(path, parse(path, text));
    const go = (params = "u: string", before = "") =>
      `export function go(${params}) {\n${before}  dismissLinkFailure();\n  void openUrl(u).catch(() => setFailed(true));\n}`;
    const imported = 'import { dismissLinkFailure } from "../lib/linkFailure";\n';
    expect(read(imported + go())).toEqual([]);
    expect(
      read(
        `import { dismissLinkFailure, openLink } from "../../../lib/linkFailure.ts";\n${go()}`,
        "src/components/calendar/parts/X.tsx",
      ),
    ).toEqual([]);
    // linkFailure.ts's own declaration, there and nowhere else.
    const own = `export function dismissLinkFailure(): void {}\n${go()}`;
    expect(read(own, "src/lib/linkFailure.ts")).toEqual([]);
    for (const [bad, path] of [
      [own, "src/lib/other.ts"],
      [`const dismissLinkFailure = () => {};\n${go()}`],
      [go()],
      [`import { dismissLinkFailure } from "./linkFailure";\n${go()}`],
      [`import { dismissLinkFailure } from "../lib/notLinkFailure";\n${go()}`],
      [`import { showLinkFailure as dismissLinkFailure } from "../lib/linkFailure";\n${go()}`],
      [`import { dismissLinkFailure as d } from "../lib/linkFailure";\n${go()}`],
      [`import dismissLinkFailure from "../lib/linkFailure";\n${go()}`],
      [`import * as dismissLinkFailure from "../lib/linkFailure";\n${go()}`],
      [imported + imported + go()],
      [imported + go("u: string, dismissLinkFailure: () => void")],
      [imported + go("u: string", "  function dismissLinkFailure() {}\n")],
      [imported + go("u: string", "  const { x: dismissLinkFailure } = y;\n")],
      [`${imported}try {\n} catch (dismissLinkFailure) {}\n${go()}`],
      [`${imported}const f = dismissLinkFailure;\n${go()}`],
    ]) {
      expect(read(bad, path), bad).not.toEqual([]);
    }
  });

  it("accounts for every openUrl the parser sees, not only its calls", () => {
    const read = (text: string, path = "src/components/X.tsx") =>
      openUrlMentions(path, parse(path, text));
    expect(
      read(
        'import { eventFlags, openUrl } from "../../../lib/ipc";\nopenUrl(u);',
        "src/components/calendar/parts/X.tsx",
      ),
    ).toEqual({ calls: 1, others: [], problems: [] });
    expect(
      read(
        'import {\n  type Foo,\n  openUrl,\n} from "./ipc.ts";\nopenUrl(a);\nopenUrl(b);',
        "src/lib/X.ts",
      ),
    ).toEqual({ calls: 2, others: [], problems: [] });
    // A comment isn't code, so its mention isn't one.
    expect(
      read(
        'import { openUrl } from "./ipc";\n// `openUrl` opens it\n/**\n * Calls openUrl.\n */',
        "src/lib/X.ts",
      ),
    ).toEqual({ calls: 0, others: [], problems: [] });
    // Prose that says "import (" isn't an import, and a plain dynamic import of another module is fine.
    expect(
      read('const say = "fail to import (it)";\nconst App = lazy(() => import("./App"));').problems,
    ).toEqual([]);
    // Any other mention is named by its role and its line, for the pins.
    expect(read('import { openUrl } from "../lib/ipc";\nconst o = openUrl;').others).toEqual([
      "other: const o = openUrl;",
    ]);
    const clean = (text: string) => {
      const file = parse("src/components/X.tsx", text);
      const m = openUrlMentions("src/components/X.tsx", file);
      return m.problems.length === 0 && m.others.length === 0 && openUrlProblems(file).length === 0;
    };
    for (const bad of [
      'import * as ipc from "../lib/ipc";\nipc.openUrl(u);',
      'import * as ipc from "../lib/ipc";\nipc["open" + "Url"](u);',
      'import * as ipc from "../lib/ipc.js";\nipc["open" + "Url"](u);',
      'import * as ipc from "@/lib/IPC.mts?x";\nipc["open" + "Url"](u);',
      'import /* all */ * as ipc from "../lib/ipc";\nipc["open" + "Url"](u);',
      'const ipc = await import("../lib/ipc");\nipc.openUrl(u);',
      "const ipc = await import(`../lib/ipc`);\nipc.openUrl(u);",
      'const ipc = await import("../lib/" + "ipc");\nipc["open" + "Url"](u);',
      'const ipc = await import ("../lib/" + "ipc");\nipc["open" + "Url"](u);',
      'const ipc = await import /* c */ ("../lib/" + "ipc");\nipc["open" + "Url"](u);',
      'const m = import.meta.glob("../lib/ipc.ts", { eager: true });\nObject.values(m)[0]["open" + "Url"](u);',
      'export * from "./ipc";',
      'export { openUrl } from "./ipc";',
      'import { openUrl as open } from "../lib/ipc";\nopen(u);',
      'import { "openUrl" as open } from "../lib/ipc";\nopen(u);',
      'import { openUrl /* the opener */ as open } from "../lib/ipc";\nopen(u);',
      'import { openUrl // the opener\n  as open } from "../lib/ipc";\nopen(u);',
      'import { openUrl } from "../lib/ipc";\nconst o = openUrl;\no(u);',
      'import { openUrl } from "../lib/ipc";\n/* keep */ export const open = openUrl;',
      'import { openUrl } from "../lib/ipc";\n/**\n * x\n */ export const o = openUrl;',
      'import { openUrl } from "../lib/ipc";\n/* the opener */ const o = openUrl;\no(u);',
      'import { openUrl } from "../lib/ipc";\nconst re = /\\/\\//; const o = openUrl;',
      'import { openUrl } from "../lib/ipc";\nopenUrl?.(u);',
      'import { openUrl } from "../lib/ipc";\nopenUrl (u);',
      'import { openUrl } from "../lib/ipc";\nopenUrl.call(null, u);',
      'import { openUrl } from "../lib/ipc";\n<a onClick={() => go(openUrl)} />',
      'import { openUrl } from "./notIpc";',
      'const name = "openUrl";',
      // The parser's reading closes these too.
      'const ipc = require("../lib/ipc");\nipc["open" + "Url"](u);',
      'import ipc = require("../lib/ipc");\nipc["open" + "Url"](u);',
      'export * as ipc from "./ipc";',
      'const m = import.meta["glob"]("../lib/ipc.ts");',
      'import { openUrl } from "../lib/ipc";\nconst o = open\\u0055rl;\no(u);',
      "const name = `openUrl`;",
      "const name = /openUrl/.source;",
      // A path that comes back to lib/ipc through `.` or `..` is lib/ipc (round 9).
      'import * as ipc from "../lib/ipc/.";\nipc["open" + "Url"](u);',
      'const ipc = await import("../lib/ipc/.");\nipc["open" + "Url"](u);',
      'export * from "./ipc/.";',
      'import * as ipc from "../lib/ipc/x/..";\nipc["open" + "Url"](u);',
      'const ipc = require("..\\\\lib\\\\ipc\\\\.");\nipc["open" + "Url"](u);',
      // And one that ends in no name at all can't be named, so it fails closed.
      'import * as m from "..";',
      'const m = await import("./");',
      // The command is spelled only in openUrl (round 9).
      'export const go = (u: string) => __TAURI_INTERNALS__.invoke("open_url", { url: u });',
      "const command = `open_url`;",
    ]) {
      expect(clean(bad), bad).toBe(false);
    }
    // A path through `.` and `..` to another module is still fine.
    expect(read('const App = lazy(() => import("./pages/x/../App/."));').problems).toEqual([]);
    expect(read('import * as lib from "../lib/ipc/x/../..";').problems).toEqual([]);
    // A second wrapper of the command, in the one file that may invoke it, is a mention more.
    const wrapper = 'export const openUrl = (url: string) => invoke<void>("open_url", { url });';
    expect(read(wrapper, "src/lib/ipc.ts").others).toEqual(OPEN_URL_ELSEWHERE["src/lib/ipc.ts"]);
    for (const second of [
      'export const openInBrowser = (url: string) => invoke<void>("open_url", { url });',
      "export const openInBrowser = (url: string) => invoke<void>(`open_url`, { url });",
    ]) {
      expect(read(`${wrapper}\n${second}`, "src/lib/ipc.ts").others).toEqual([
        ...OPEN_URL_ELSEWHERE["src/lib/ipc.ts"],
        `command: ${second}`,
      ]);
    }
  });
});

/** Every file that calls `openUrl` and how many times. linkFailure.ts's is `openLink`, the notice's
 *  own; the others say a failure on their own line. */
const OPEN_URL_CALLERS: Record<string, number> = {
  "src/lib/linkFailure.ts": 1,
  "src/components/SignInStrip.tsx": 1,
  "src/components/calendar/edit/MeetField.tsx": 1,
  "src/components/calendar/parts/CalendarEventPopover.tsx": 2,
  "src/components/calendar/parts/useMailLink.ts": 1,
};

/** Every other `openUrl` the parser sees, by file, as its role and its line: ipc.ts's definition,
 *  and the right-click menu's "Open link" action (contextMenu.ts's type member and its call), which
 *  ContextMenuHost fills with `openLink`. And the one literal that spells the command, `open_url`:
 *  openUrl's, so it is the only wrapper of it. Comments aren't code, so none is here. */
const OPEN_URL_ELSEWHERE: Record<string, string[]> = {
  "src/lib/ipc.ts": [
    'export const: export const openUrl = (url: string) => invoke<void>("open_url", { url });',
    'command: export const openUrl = (url: string) => invoke<void>("open_url", { url });',
  ],
  "src/lib/contextMenu.ts": [
    "type member: actions: { openUrl: (url: string) => void; copy: (text: string) => void },",
    'property access: { label: "Open link", onSelect: () => actions.openUrl(link) },',
  ],
  "src/components/ui/ContextMenuHost.tsx": ["property name: openUrl: openLink,"],
};

describe("callers of openUrl", () => {
  it("are the ones with a line of their own, each call taking the notice down, none silent", () => {
    const files = new Map([...SOURCES].map(([path, text]) => [path, parse(path, text)] as const));
    const mentions = [...files].map(([path, file]) => [path, openUrlMentions(path, file)] as const);
    const calls = Object.fromEntries(
      mentions.map(([path, m]) => [path, m.calls] as const).filter(([, n]) => n > 0),
    );
    expect(calls).toEqual(OPEN_URL_CALLERS);
    const others = Object.fromEntries(
      mentions.map(([path, m]) => [path, m.others] as const).filter(([, o]) => o.length > 0),
    );
    expect(others).toEqual(OPEN_URL_ELSEWHERE);
    for (const [path, m] of mentions) expect(m.problems, path).toEqual([]);
    for (const path of Object.keys(OPEN_URL_CALLERS)) {
      expect(openUrlProblems(files.get(path)!), path).toEqual([]);
      expect(dismissProblems(path, files.get(path)!), path).toEqual([]);
    }
    // And the openers that use the notice still reach it.
    for (const path of [
      "src/lib/useExternalLinks.ts",
      "src/components/ui/ContextMenuHost.tsx",
      "src/components/NoteEditor.tsx",
      "src/components/RichNoteEditor.tsx",
      "src/components/calendar/edit/DescriptionEditor.tsx",
      "src/components/backup/ProtonDriveSection.tsx",
    ]) {
      // Called, or handed on as the menu's opener; an import alone isn't either.
      const reaches = nodesOf(files.get(path)!).some(
        (n) =>
          ts.isIdentifier(n) &&
          n.text === "openLink" &&
          (callOf(n) !== undefined ||
            (ts.isPropertyAssignment(n.parent) && n.parent.initializer === n)),
      );
      expect(reaches, path).toBe(true);
    }
  });
});
