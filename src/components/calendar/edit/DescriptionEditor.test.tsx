// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The description editor (C19 §7, C19d): the box is seeded without parsing anything, reports what
// it shows (only when that changed), takes paste and drops from elsewhere as words, runs one literal
// command per button or shortcut, keeps links from opening PM's window, and stands still while a
// save is out. jsdom has no editing engine: `execCommand` and `queryCommandState` are stubbed, and
// typing is the box's nodes changed by hand, then the input event the engine would fire.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import { spyOnHtmlSinks, type HtmlSinkSpy } from "../../../lib/calendarEdit/htmlSinkSpy";
import { FORMAT_BUTTONS, formatKey } from "../../../lib/calendarEdit/formatKeys";

const openUrl = vi.fn<(url: string) => Promise<void>>();
vi.mock("../../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  openUrl: (url: string) => openUrl(url),
}));

let system = "slate";
vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system, depth: "standard" }),
}));

import { DescriptionEditor, DescriptionView } from "./DescriptionEditor";
import { NOT_AN_ADDRESS } from "./LinkPanel";

let sinks: HtmlSinkSpy;
let exec: MockInstance<Document["execCommand"]>;

beforeEach(() => {
  system = "slate";
  // Before render: the views write while they render (critique C1-M8).
  sinks = spyOnHtmlSinks();
  exec = vi.spyOn(document, "execCommand");
});

afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(document, "queryCommandState");
  // Before cleanup, whose own emptying of the page would count.
  sinks.restore();
  cleanup();
  openUrl.mockReset();
});

/** The editor, named by a "Description" text as the editors name it. */
function editor(value: string, over: { disabled?: boolean; onMail?: (t: string) => void } = {}) {
  const onChange = vi.fn<(value: string) => void>();
  const onMail = over.onMail ?? vi.fn();
  const view = (v: string, disabled = over.disabled ?? false) => (
    <div>
      <span id="desc-label">Description</span>
      <DescriptionEditor
        value={v}
        onChange={onChange}
        disabled={disabled}
        labelledBy="desc-label"
        onMail={onMail}
      />
    </div>
  );
  const { rerender } = render(view(value));
  return {
    box: screen.getByRole("textbox", { name: "Description" }),
    onChange,
    onMail,
    rerender: (v: string, disabled?: boolean) => rerender(view(v, disabled)),
  };
}

const toolbar = () => screen.getByRole("toolbar", { name: "Description formatting" });
const button = (name: string) => within(toolbar()).getByRole("button", { name });
/** The link panel's fields (the toolbar's Link button has the same name). */
const linkField = () => screen.getByRole<HTMLInputElement>("textbox", { name: "Link" });
const textField = () => screen.getByRole<HTMLInputElement>("textbox", { name: "Text" });

/** What the user typed, as the engine leaves the box, then the input event it fires. */
function typeInto(box: HTMLElement, ...nodes: (Node | string)[]) {
  box.replaceChildren(...nodes);
  fireEvent.input(box);
}

/** An element made by hand, never by parsing. */
function h(tag: string, attrs: Record<string, string> = {}, ...children: (Node | string)[]) {
  const made = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs)) made.setAttribute(name, value);
  made.append(...children);
  return made;
}

/** The selection on `node`'s contents (or collapsed at its start), as a click or a drag leaves it. */
function select(node: Node, collapsed = false) {
  const range = document.createRange();
  range.selectNodeContents(node);
  if (collapsed) range.collapse(true);
  const sel = document.getSelection() as Selection;
  sel.removeAllRanges();
  sel.addRange(range);
  act(() => {
    document.dispatchEvent(new Event("selectionchange"));
  });
}

/** The commands run, apart from the one focus runs. */
const commands = () => exec.mock.calls.map((c) => c[0]).filter((c) => c !== "styleWithCSS");

describe("DescriptionEditor", () => {
  it("seeds_the_box_without_parsing_html", () => {
    const { box } = editor(
      '<b>Agenda</b> one <a href="https://example.com" target="_blank">site</a>',
    );
    expect(box.querySelector("b")?.textContent).toBe("Agenda");
    const link = box.querySelector("a") as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe("https://example.com");
    // Never a target: the app-wide opener must not open a link being edited.
    expect(link.hasAttribute("target")).toBe(false);
    expect(box.textContent).toBe("Agenda one site");
    expect(sinks.sinks).toEqual([]);
  });

  it("typing_reports_plain_lines_or_formatting", () => {
    const { box, onChange } = editor("");
    typeInto(box, "a", h("br"), "b");
    expect(onChange).toHaveBeenLastCalledWith("a\nb");
    typeInto(box, h("b", {}, "x"));
    expect(onChange).toHaveBeenLastCalledWith("<b>x</b>");
    // Words that look like formatting stay words.
    typeInto(box, "<b>");
    expect(onChange).toHaveBeenLastCalledWith("&lt;b&gt;");
    expect(onChange).toHaveBeenCalledTimes(3);
  });

  // Critique C1-M2: an untouched box reports nothing, so the draft keeps Google's bytes.
  it.each([
    "<html-blob>Hi<br>there</html-blob>",
    "<strong>x</strong>",
    "<br>",
    "<p><br></p>",
    "<html-blob><br></html-blob>",
    "\n",
    " ",
    "",
  ])("leaving_an_untouched_description_reports_what_it_showed: %j", (value) => {
    const { box, onChange } = editor(value);
    box.focus();
    fireEvent.input(box);
    fireEvent.blur(box);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("an_emptied_box_reports_nothing_to_show", () => {
    const { box, onChange } = editor("Notes");
    // What engines leave when everything is deleted.
    typeInto(box, h("br"));
    expect(onChange).toHaveBeenLastCalledWith("");
    // Another empty shape still shows nothing: no change to report.
    typeInto(box, h("p", {}, h("br")));
    typeInto(box, h("ul", {}, h("li", {}, " ")));
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("buttons_send_only_their_own_commands", () => {
    const { box } = editor("words");
    fireEvent.focus(box);
    // Bold, italic and underline as elements, not styled spans.
    expect(exec).toHaveBeenCalledWith("styleWithCSS", false, "false");
    select(box);
    for (const name of ["Bold", "Italic", "Underline", "Bulleted list", "Numbered list"]) {
      fireEvent.click(button(name));
    }
    fireEvent.click(button("Remove formatting"));
    expect(commands()).toEqual([
      "bold",
      "italic",
      "underline",
      "insertUnorderedList",
      "insertOrderedList",
      "removeFormat",
    ]);
    // Link opens its panel, and runs nothing until it's applied.
    fireEvent.click(button("Link"));
    expect(linkField()).toBeTruthy();
    expect(commands()).toHaveLength(6);
    expect(sinks.commands).not.toContain("insertHTML");
  });

  // A command an engine doesn't have (it answers false) changes nothing else.
  it("a_command_the_engine_lacks_reports_nothing", () => {
    const { box, onChange } = editor("words");
    select(box);
    box.replaceChildren(h("b", {}, "words"));
    fireEvent.click(button("Bold"));
    expect(onChange).not.toHaveBeenCalled();
    // One it ran reports what the box shows now.
    exec.mockReturnValue(true);
    fireEvent.click(button("Bold"));
    expect(onChange).toHaveBeenLastCalledWith("<b>words</b>");
  });

  it("a_button_reached_by_tab_works_where_the_selection_last_was", () => {
    const { box } = editor("one two");
    const words = box.firstChild as Text;
    const range = document.createRange();
    range.setStart(words, 4);
    range.setEnd(words, 7);
    const sel = document.getSelection() as Selection;
    sel.removeAllRanges();
    sel.addRange(range);
    act(() => {
      document.dispatchEvent(new Event("selectionchange"));
    });
    // Focus leaves for the toolbar, and the selection with it.
    sel.removeAllRanges();
    button("Bold").focus();
    fireEvent.click(button("Bold"));
    expect(document.activeElement).toBe(box);
    const now = sel.getRangeAt(0);
    expect([now.startContainer, now.startOffset, now.endOffset]).toEqual([words, 4, 7]);
    expect(commands()).toEqual(["bold"]);
  });

  it("buttons_keep_the_selection", () => {
    editor("words");
    for (const { label } of FORMAT_BUTTONS) {
      // False: the mouse-down's default (taking focus and the selection) was prevented.
      expect(fireEvent.mouseDown(button(label))).toBe(false);
    }
  });

  it("shortcuts_format_on_every_platform", () => {
    const { box } = editor("words");
    select(box);
    const seen = vi.fn();
    window.addEventListener("keydown", seen);
    try {
      const press = (code: string, shiftKey = false) =>
        fireEvent.keyDown(box, { key: "x", code, ctrlKey: true, shiftKey });
      for (const [code, shift] of [
        ["KeyB", false],
        ["KeyI", false],
        ["KeyU", false],
        ["Digit8", true],
        ["Digit7", true],
        ["Backslash", false],
      ] as const) {
        expect(press(code, shift), code).toBe(false);
      }
      expect(commands()).toEqual([
        "bold",
        "italic",
        "underline",
        "insertUnorderedList",
        "insertOrderedList",
        "removeFormat",
      ]);
      // Ctrl+K opens the link panel, and never reaches the command palette's window listener.
      expect(press("KeyK")).toBe(false);
      expect(linkField()).toBeTruthy();
      expect(seen).not.toHaveBeenCalled();
      // Tab leaves the box (no trap), and other keys are the engine's.
      expect(fireEvent.keyDown(box, { key: "Tab", code: "Tab" })).toBe(true);
      expect(fireEvent.keyDown(box, { key: "z", code: "KeyZ", ctrlKey: true })).toBe(true);
      expect(fireEvent.keyDown(box, { key: "b", code: "KeyB", ctrlKey: true, altKey: true })).toBe(
        true,
      );
    } finally {
      window.removeEventListener("keydown", seen);
    }
  });

  it("paste_brings_in_text_only", () => {
    const { box, onChange } = editor("");
    select(box, true);
    const clipboardData = {
      getData: (type: string) => (type === "text/plain" ? "plain words" : "<b>rich</b>"),
    };
    expect(fireEvent.paste(box, { clipboardData })).toBe(false);
    expect(exec).toHaveBeenCalledWith("insertText", false, "plain words");
    // An engine without the command: the words go in as a text node, and are reported.
    expect(box.querySelector("b")).toBeNull();
    expect(onChange).toHaveBeenLastCalledWith("plain words");
    expect(sinks.sinks).toEqual([]);
  });

  // C19 review 1: WebKit's insert splits lines at a line feed only, so a CRLF file's lines would
  // each keep a carriage return, which a save makes a line of its own.
  it.each([
    ["a\r\nb\r\nc", "a\nb\nc"],
    ["a\r\n\r\nb", "a\n\nb"],
    ["a\rb", "a\nb"],
  ])("pasted_line_endings_are_line_feeds: %j", (pasted, inserted) => {
    const { box } = editor("");
    select(box, true);
    fireEvent.paste(box, { clipboardData: { getData: () => pasted } });
    expect(exec).toHaveBeenCalledWith("insertText", false, inserted);
  });

  // Critique C2-L9: text dropped from elsewhere comes in as words; a drag inside the box is the
  // engine's move.
  it("a_drop_from_elsewhere_brings_in_its_words_only", () => {
    const { box, onChange } = editor("");
    const dataTransfer = {
      getData: (type: string) => (type === "text/plain" ? "dropped" : "<i>dropped</i>"),
    };
    expect(fireEvent.drop(box, { dataTransfer, clientX: 1, clientY: 1 })).toBe(false);
    expect(exec).toHaveBeenCalledWith("insertText", false, "dropped");
    expect(onChange).toHaveBeenLastCalledWith("dropped");
    expect(document.activeElement).toBe(box);
    exec.mockClear();
    fireEvent.dragStart(box);
    expect(fireEvent.drop(box, { dataTransfer })).toBe(true);
    expect(exec).not.toHaveBeenCalledWith("insertText", false, "dropped");
    fireEvent.dragEnd(box);
    expect(fireEvent.drop(box, { dataTransfer })).toBe(false);
  });

  it("a_paste_that_skips_the_paste_event_is_text_too", async () => {
    const { box } = editor("");
    const input = (inputType: string) => {
      const e = new InputEvent("beforeinput", { inputType, bubbles: true, cancelable: true });
      // jsdom has no DataTransfer.
      Object.defineProperty(e, "dataTransfer", {
        value: { getData: (type: string) => (type === "text/plain" ? "middle" : "<b>x</b>") },
      });
      box.dispatchEvent(e);
      return e;
    };
    for (const kind of ["insertFromPaste", "insertFromPasteAsQuotation", "insertFromYank"]) {
      exec.mockClear();
      expect(input(kind).defaultPrevented, kind).toBe(true);
      await act(async () => {});
      expect(exec).toHaveBeenCalledWith("insertText", false, "middle");
    }
    // A drop from elsewhere is the drop handler's; one dragged inside the box is the engine's.
    expect(input("insertFromDrop").defaultPrevented).toBe(true);
    fireEvent.dragStart(box);
    expect(input("insertFromDrop").defaultPrevented).toBe(false);
    // Typing is the engine's.
    expect(input("insertText").defaultPrevented).toBe(false);
  });

  it("a_new_value_reseeds_the_box_and_typing_does_not", () => {
    const { box, onChange, rerender } = editor("<b>a</b>");
    const typed = h("b", {}, "ab");
    typeInto(box, typed);
    expect(onChange).toHaveBeenLastCalledWith("<b>ab</b>");
    // The draft takes what the box reported: the box keeps the engine's own nodes (and its Undo).
    rerender("<b>ab</b>");
    expect(box.firstChild).toBe(typed);
    // A value that shows the same, written another way, is no reason either.
    rerender("<strong>ab</strong>");
    expect(box.firstChild).toBe(typed);
    // Google's copy after a conflict is.
    rerender("<i>from Google</i>");
    expect(box.firstChild).not.toBe(typed);
    expect(box.querySelector("i")?.textContent).toBe("from Google");
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("pressed_buttons_follow_the_selection", () => {
    Object.defineProperty(document, "queryCommandState", {
      configurable: true,
      value: (command: string) => command === "bold" || command === "insertOrderedList",
    });
    const { box } = editor('<b>bold</b> <a href="https://example.com">site</a>');
    select(box.querySelector("b") as Node);
    for (const name of ["Bold", "Numbered list"]) {
      expect(button(name).getAttribute("aria-pressed"), name).toBe("true");
      expect(button(name).className, name).toContain("bg-accent");
    }
    for (const name of ["Italic", "Underline", "Bulleted list"]) {
      expect(button(name).getAttribute("aria-pressed"), name).toBe("false");
    }
    // Link and Remove formatting aren't toggles: Link says whether its panel is open.
    expect(button("Link").hasAttribute("aria-pressed")).toBe(false);
    expect(button("Link").getAttribute("aria-expanded")).toBe("false");
    expect(button("Remove formatting").hasAttribute("aria-pressed")).toBe(false);
    expect(button("Link").className).not.toContain("bg-accent");
    // The caret in a link shows Link pressed.
    select(box.querySelector("a") as Node, true);
    expect(button("Link").className).toContain("bg-accent");
    // An engine that can't say shows nothing pressed.
    Reflect.deleteProperty(document, "queryCommandState");
    select(box.querySelector("b") as Node);
    expect(button("Bold").getAttribute("aria-pressed")).toBe("false");
    // A selection outside the box changes nothing.
    const elsewhere = h("p", {}, "outside");
    document.body.append(elsewhere);
    Object.defineProperty(document, "queryCommandState", { configurable: true, value: () => true });
    select(elsewhere);
    expect(button("Bold").getAttribute("aria-pressed")).toBe("false");
    elsewhere.remove();
  });

  // Critique C1-L8: the attribute, not jsdom's (missing) isContentEditable.
  it("standing_still_while_saving", () => {
    const { box, rerender, onChange } = editor("words", { disabled: true });
    expect(box.getAttribute("contenteditable")).toBe("false");
    expect(box.getAttribute("aria-disabled")).toBe("true");
    expect(box.tabIndex).toBe(-1);
    for (const b of within(toolbar()).getAllByRole("button")) {
      expect((b as HTMLButtonElement).disabled).toBe(true);
    }
    select(box);
    expect(fireEvent.keyDown(box, { key: "b", code: "KeyB", ctrlKey: true })).toBe(false);
    expect(commands()).toEqual([]);
    expect(fireEvent.paste(box, { clipboardData: { getData: () => "x" } })).toBe(false);
    expect(fireEvent.drop(box, { dataTransfer: { getData: () => "x" } })).toBe(false);
    expect(exec).not.toHaveBeenCalledWith("insertText", false, "x");
    expect(onChange).not.toHaveBeenCalled();
    // Open again after the save.
    rerender("words", false);
    expect(box.getAttribute("contenteditable")).toBe("true");
    expect(box.hasAttribute("aria-disabled")).toBe(false);
    expect(box.tabIndex).toBe(0);
  });

  it("the_link_panel_closes_when_a_save_starts", () => {
    const { box, rerender } = editor("words");
    select(box);
    fireEvent.click(button("Link"));
    expect(linkField()).toBeTruthy();
    rerender("words", true);
    expect(screen.queryByRole("textbox", { name: "Link" })).toBeNull();
    rerender("words", false);
    expect(screen.queryByRole("textbox", { name: "Link" })).toBeNull();
  });

  it("named_and_reachable", () => {
    const { box } = editor("words");
    expect(box.getAttribute("aria-multiline")).toBe("true");
    expect(box.getAttribute("aria-labelledby")).toBe("desc-label");
    expect(box.tabIndex).toBe(0);
    expect(toolbar().getAttribute("data-help")).toBe("calendar-description-format");
    // Google's seven, in order, each named by its label, its shortcut said apart (critique C2-M4).
    const buttons = within(toolbar()).getAllByRole("button");
    expect(buttons.map((b) => b.getAttribute("aria-label"))).toEqual([
      "Bold",
      "Italic",
      "Underline",
      "Bulleted list",
      "Numbered list",
      "Link",
      "Remove formatting",
    ]);
    expect(buttons.map((b) => b.getAttribute("aria-keyshortcuts"))).toEqual([
      "Control+B",
      "Control+I",
      "Control+U",
      "Control+Shift+8",
      "Control+Shift+7",
      "Control+K",
      "Control+\\",
    ]);
    // No native tooltip beside the drawn one.
    expect(buttons.every((b) => b.getAttribute("title") === "")).toBe(true);
    // One Tab stop; the arrows, Home and End move along it.
    expect(buttons.map((b) => b.tabIndex)).toEqual([0, -1, -1, -1, -1, -1, -1]);
    buttons[0].focus();
    expect(fireEvent.keyDown(buttons[0], { key: "ArrowRight" })).toBe(false);
    expect(document.activeElement).toBe(buttons[1]);
    expect(buttons.map((b) => b.tabIndex)).toEqual([-1, 0, -1, -1, -1, -1, -1]);
    fireEvent.keyDown(buttons[1], { key: "End" });
    expect(document.activeElement).toBe(buttons[6]);
    fireEvent.keyDown(buttons[6], { key: "ArrowRight" });
    expect(document.activeElement).toBe(buttons[0]);
    fireEvent.keyDown(buttons[0], { key: "ArrowLeft" });
    expect(document.activeElement).toBe(buttons[6]);
    fireEvent.keyDown(buttons[6], { key: "Home" });
    expect(document.activeElement).toBe(buttons[0]);
    // Other keys are left alone.
    expect(fireEvent.keyDown(buttons[0], { key: "ArrowDown" })).toBe(true);
  });

  it("tooltips_say_the_label_and_the_shortcut", () => {
    vi.useFakeTimers();
    try {
      editor("words");
      fireEvent.focus(button("Bulleted list"));
      act(() => {
        vi.advanceTimersByTime(200);
      });
      expect(screen.getByRole("tooltip").textContent).toBe("Bulleted list · Ctrl+Shift+8");
    } finally {
      vi.useRealTimers();
    }
  });

  it("the_terminal_system_types_in_mono", () => {
    expect(editor("").box.className).not.toContain("font-mono");
    cleanup();
    system = "terminal";
    expect(editor("").box.className).toContain("font-mono");
  });

  // Critique C2-H3: no link in the box ever navigates PM's window; Ctrl+click opens a web or mail
  // address the link rule takes, as the formatted view does.
  it("links_in_the_box_open_only_on_ctrl_click", async () => {
    openUrl.mockResolvedValue();
    const { box, onMail } = editor(
      '<a href="https://example.com/x">web</a> <a href="mailto:ann@example.com">mail</a> ' +
        '<a href="tel:+15550100">phone</a>',
    );
    const [web, mail, phone] = [...box.querySelectorAll("a")];
    for (const link of [web, mail, phone]) {
      expect(fireEvent.click(link)).toBe(false);
      expect(fireEvent.click(link, { shiftKey: true })).toBe(false);
      expect(fireEvent(link, new MouseEvent("auxclick", { bubbles: true, cancelable: true }))).toBe(
        false,
      );
    }
    expect(openUrl).not.toHaveBeenCalled();
    expect(onMail).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(web.firstChild as Node, { ctrlKey: true });
    });
    expect(openUrl).toHaveBeenCalledExactlyOnceWith("https://example.com/x");
    fireEvent.click(mail, { ctrlKey: true });
    expect(onMail).toHaveBeenCalledExactlyOnceWith("mailto:ann@example.com");
    // A phone link is kept, never opened.
    fireEvent.click(phone, { ctrlKey: true });
    expect(openUrl).toHaveBeenCalledTimes(1);
    expect(onMail).toHaveBeenCalledTimes(1);
    // Other clicks in the box are the engine's (the caret).
    expect(fireEvent.click(box)).toBe(true);
  });

  it("links_in_the_box_never_open_while_saving", () => {
    const { box } = editor('<a href="https://example.com/x">web</a>', { disabled: true });
    const link = box.querySelector("a") as HTMLAnchorElement;
    expect(fireEvent.click(link)).toBe(false);
    expect(fireEvent(link, new MouseEvent("auxclick", { bubbles: true, cancelable: true }))).toBe(
      false,
    );
  });

  it("never_writes_html_into_the_page", () => {
    exec.mockReturnValue(true);
    const { box, rerender } = editor(
      "<html-blob><b>a</b><ol><li>one</li></ol><p>two</p>" +
        '<a href="https://example.com">x</a><script>alert(1)</script></html-blob>',
    );
    typeInto(box, h("b", {}, "a"), h("img", { src: "x", onerror: "alert(1)" }), "<script>");
    select(box);
    for (const { label } of FORMAT_BUTTONS) {
      if (label !== "Link") fireEvent.click(button(label));
    }
    fireEvent.paste(box, { clipboardData: { getData: () => "<img src=x onerror=alert(1)>" } });
    rerender("<i>again</i>");
    expect(sinks.sinks).toEqual([]);
    expect(sinks.commands).not.toContain("insertHTML");
  });
});

// --- the link panel, from the editor (C19 §7.4) ---

describe("DescriptionEditor's link panel", () => {
  it("links_the_selection_to_what_was_typed", () => {
    const { box, onChange } = editor("see the site");
    const words = box.firstChild as Text;
    const range = document.createRange();
    range.setStart(words, 8);
    range.setEnd(words, 12);
    const sel = document.getSelection() as Selection;
    sel.removeAllRanges();
    sel.addRange(range);
    fireEvent.keyDown(box, { key: "k", code: "KeyK", ctrlKey: true });
    expect(textField().value).toBe("site");
    // The address takes focus first.
    expect(document.activeElement).toBe(linkField());
    expect(button("Link").getAttribute("aria-expanded")).toBe("true");
    fireEvent.change(linkField(), { target: { value: "example.com" } });
    // A link the engine made, as createLink leaves it.
    exec.mockImplementation((command) => {
      if (command === "createLink") {
        box.replaceChildren("see the ", h("a", { href: "https://example.com" }, "site"));
      }
      return true;
    });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(commands()).toEqual(["createLink"]);
    expect(exec).toHaveBeenCalledWith("createLink", false, "https://example.com");
    expect(onChange).toHaveBeenLastCalledWith('see the <a href="https://example.com">site</a>');
    expect(screen.queryByRole("textbox", { name: "Link" })).toBeNull();
    expect(document.activeElement).toBe(box);
  });

  // Critique C1-M7: new words, then the selection back over them from where they start, with no
  // characters counted.
  it("new_words_are_typed_then_linked", () => {
    const { box } = editor("ab");
    const words = box.firstChild as Text;
    select(words, true);
    const sel = document.getSelection() as Selection;
    sel.collapse(words, 1);
    const extent = vi.spyOn(Selection.prototype, "setBaseAndExtent");
    exec.mockImplementation((command, _ui, value) => {
      if (command === "insertText") {
        // As the engine types: the words in at the caret, the caret after them.
        words.insertData(1, value ?? "");
        sel.collapse(words, 1 + (value ?? "").length);
      }
      return true;
    });
    fireEvent.click(button("Link"));
    fireEvent.change(textField(), { target: { value: "PM" } });
    fireEvent.change(linkField(), { target: { value: "someone@example.com" } });
    fireEvent.keyDown(linkField(), { key: "Enter" });
    expect(commands()).toEqual(["insertText", "createLink"]);
    expect(exec).toHaveBeenCalledWith("insertText", false, "PM");
    expect(exec).toHaveBeenLastCalledWith("createLink", false, "mailto:someone@example.com");
    expect(extent).toHaveBeenCalledWith(words, 1, words, 3);
  });

  // With no words at all, a mail link's words are its address, never "mailto:".
  it("an_empty_text_makes_the_address_its_words", () => {
    const { box } = editor("ab");
    select(box.firstChild as Node, true);
    exec.mockReturnValue(true);
    fireEvent.click(button("Link"));
    fireEvent.change(linkField(), { target: { value: "ann@example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(exec).toHaveBeenCalledWith("insertText", false, "ann@example.com");
    expect(exec).toHaveBeenLastCalledWith("createLink", false, "mailto:ann@example.com");
  });

  // C19 review 1: linked at a caret, the caret comes back after the link, so the next key types
  // after it rather than over it; and the link starts at its words, not at the space before them,
  // which both engines rewrite as they type (pulling a kept range back over it).
  it("a_link_made_at_the_caret_leaves_the_caret_after_it", () => {
    // A no-break space (U+00A0), as the engine types a space at the end of a line.
    const { box, onChange } = editor(`See${String.fromCodePoint(0xa0)}`);
    const words = box.firstChild as Text;
    const sel = document.getSelection() as Selection;
    select(words, true);
    sel.collapse(words, 4);
    exec.mockImplementation((command, _ui, value) => {
      if (command === "insertText") {
        // As the engines type after a no-break space: it becomes a space, then the words go in.
        words.replaceData(3, 1, " ");
        words.insertData(4, value ?? "");
        sel.collapse(words, 4 + (value ?? "").length);
      }
      if (command === "createLink") {
        // As the engines link: the selection's words wrapped, and left selected.
        const link = document.createElement("a");
        link.setAttribute("href", value ?? "");
        sel.getRangeAt(0).surroundContents(link);
        sel.selectAllChildren(link);
      }
      return true;
    });
    fireEvent.click(button("Link"));
    fireEvent.change(textField(), { target: { value: "the site" } });
    fireEvent.change(linkField(), { target: { value: "example.com" } });
    fireEvent.keyDown(linkField(), { key: "Enter" });
    expect(onChange).toHaveBeenLastCalledWith('See <a href="https://example.com">the site</a>');
    expect(sel.isCollapsed).toBe(true);
    const caret = sel.getRangeAt(0);
    const link = box.querySelector("a") as HTMLAnchorElement;
    // After the link's words, not inside them or before.
    const after = document.createRange();
    after.selectNodeContents(link);
    expect(caret.compareBoundaryPoints(Range.END_TO_END, after)).toBeGreaterThanOrEqual(0);
  });

  // A selection linked keeps its selection (critique C2-M5).
  it("a_link_made_on_a_selection_keeps_it", () => {
    const { box } = editor("see the site");
    const words = box.firstChild as Text;
    const range = document.createRange();
    range.setStart(words, 8);
    range.setEnd(words, 12);
    const sel = document.getSelection() as Selection;
    sel.removeAllRanges();
    sel.addRange(range);
    exec.mockReturnValue(true);
    fireEvent.click(button("Link"));
    fireEvent.change(linkField(), { target: { value: "example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(sel.isCollapsed).toBe(false);
  });

  // C19 review 1: the panel opened again on another selection reads that one's words; Link's own
  // button closes it, as its aria-expanded says.
  it("the_panel_opened_again_reads_the_new_selection", () => {
    const { box } = editor("alpha beta");
    const words = box.firstChild as Text;
    const sel = document.getSelection() as Selection;
    const pick = (from: number, to: number) => {
      const range = document.createRange();
      range.setStart(words, from);
      range.setEnd(words, to);
      sel.removeAllRanges();
      sel.addRange(range);
    };
    pick(0, 5);
    fireEvent.click(button("Link"));
    expect(textField().value).toBe("alpha");
    // Back in the box, another selection, the shortcut again.
    pick(6, 10);
    fireEvent.keyDown(box, { key: "k", code: "KeyK", ctrlKey: true });
    expect(textField().value).toBe("beta");
    exec.mockReturnValue(true);
    fireEvent.change(linkField(), { target: { value: "example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    // The words already there: none typed over them.
    expect(commands()).toEqual(["createLink"]);
    // The button closes an open panel.
    fireEvent.click(button("Link"));
    expect(button("Link").getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(button("Link"));
    expect(button("Link").getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("textbox", { name: "Link" })).toBeNull();
  });

  it("a_link_is_opened_whole_and_can_be_removed", () => {
    const { box, onChange } = editor('one <a href="https://example.com/a">the site</a> two');
    const link = box.querySelector("a") as HTMLAnchorElement;
    select(link.firstChild as Node, true);
    fireEvent.click(button("Link"));
    expect(textField().value).toBe("the site");
    expect(linkField().value).toBe("https://example.com/a");
    // The whole link is selected for the command.
    let selected: [Node, number, number] | null = null;
    exec.mockImplementation((command) => {
      if (command === "unlink") {
        const range = (document.getSelection() as Selection).getRangeAt(0);
        selected = [range.startContainer, range.startOffset, range.endOffset];
        box.replaceChildren("one the site two");
      }
      return true;
    });
    fireEvent.click(screen.getByRole("button", { name: "Remove link" }));
    expect(commands()).toEqual(["unlink"]);
    expect(selected).toEqual([link, 0, 1]);
    expect(onChange).toHaveBeenLastCalledWith("one the site two");
    expect(document.activeElement).toBe(box);
  });

  // Critique C2-M5: Cancel and Escape put focus and the selection back in the box too.
  it.each(["Cancel", "Escape"])("closing_the_panel_by_%s_puts_the_caret_back", (how) => {
    const { box } = editor("see the site");
    const words = box.firstChild as Text;
    const range = document.createRange();
    range.setStart(words, 4);
    range.setEnd(words, 7);
    const sel = document.getSelection() as Selection;
    sel.removeAllRanges();
    sel.addRange(range);
    const seen = vi.fn();
    window.addEventListener("keydown", seen);
    try {
      fireEvent.click(button("Link"));
      if (how === "Cancel") fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
      else fireEvent.keyDown(linkField(), { key: "Escape" });
      expect(seen).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", seen);
    }
    expect(screen.queryByRole("textbox", { name: "Link" })).toBeNull();
    expect(document.activeElement).toBe(box);
    const now = sel.getRangeAt(0);
    expect([now.startContainer, now.startOffset, now.endOffset]).toEqual([words, 4, 7]);
    expect(commands()).toEqual([]);
  });

  it("an_address_that_isnt_one_is_refused_in_the_panel", () => {
    const { box } = editor("words");
    select(box);
    fireEvent.click(button("Link"));
    fireEvent.change(linkField(), { target: { value: "javascript:alert(1)" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(screen.getByRole("alert").textContent).toBe(NOT_AN_ADDRESS);
    expect(commands()).toEqual([]);
  });
});

describe("DescriptionView", () => {
  it("shows Google's copy, named and described, with Replace only when offered", () => {
    const onReplace = vi.fn();
    const { rerender } = render(
      <div>
        <span id="desc-label">Description</span>
        <p id="why">Why</p>
        <DescriptionView
          doc={[{ kind: "b", children: [{ kind: "text", text: "Room" }] }]}
          labelledBy="desc-label"
          describedBy="why"
          onReplace={onReplace}
          onMail={vi.fn()}
        />
      </div>,
    );
    const view = screen.getByRole("group", { name: "Description" });
    expect(view.getAttribute("aria-describedby")).toBe("why");
    expect(view.querySelector("b")?.textContent).toBe("Room");
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    expect(onReplace).toHaveBeenCalledTimes(1);
    rerender(
      <div>
        <span id="desc-label">Description</span>
        <DescriptionView doc={[]} labelledBy="desc-label" onMail={vi.fn()} />
      </div>,
    );
    expect(screen.queryByRole("button", { name: "Replace with plain text…" })).toBeNull();
    expect(
      screen.getByRole("group", { name: "Description" }).hasAttribute("aria-describedby"),
    ).toBe(false);
    expect(sinks.sinks).toEqual([]);
  });

  // C19 review 1: a middle-click sends `auxclick`, never `click`; on a mail link, its default
  // loads the address in PM's own window. A web link opens in the browser as before.
  it("a middle-click never loads a mail link inside PM's window", () => {
    const onMail = vi.fn();
    render(
      <div>
        <span id="desc-label">Description</span>
        <DescriptionView
          doc={[
            {
              kind: "a",
              href: "mailto:ann@example.com",
              children: [{ kind: "text", text: "Email us" }],
            },
            { kind: "text", text: " or bob@example.com, or https://example.com" },
          ]}
          labelledBy="desc-label"
          onMail={onMail}
        />
      </div>,
    );
    const middle = (target: Element) =>
      target.dispatchEvent(
        new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }),
      );
    const view = screen.getByRole("group", { name: "Description" });
    const mails = view.querySelectorAll('a[href^="mailto:"]');
    expect(mails.length).toBe(2);
    for (const mail of mails) expect(middle(mail)).toBe(false);
    expect(onMail).not.toHaveBeenCalled();
    expect(openUrl).not.toHaveBeenCalled();
    // A web link opens in the browser, never in PM's window (review 2).
    openUrl.mockResolvedValue(undefined);
    expect(middle(view.querySelector('a[href="https://example.com"]') as Element)).toBe(false);
    expect(openUrl).toHaveBeenCalledExactlyOnceWith("https://example.com");
  });
});

describe("formatKey", () => {
  const key = (
    code: string,
    mods: Partial<Record<"ctrl" | "meta" | "alt" | "shift", boolean>>,
  ) => ({
    code,
    ctrlKey: !!mods.ctrl,
    metaKey: !!mods.meta,
    altKey: !!mods.alt,
    shiftKey: !!mods.shift,
  });

  it("reads_ctrl_elsewhere_and_command_on_a_mac", () => {
    for (const [code, shift, name] of [
      ["KeyB", false, "bold"],
      ["KeyI", false, "italic"],
      ["KeyU", false, "underline"],
      ["Digit8", true, "bulleted"],
      ["Digit7", true, "numbered"],
      ["KeyK", false, "link"],
      ["Backslash", false, "clear"],
    ] as const) {
      expect(formatKey(key(code, { ctrl: true, shift }), false), code).toBe(name);
      expect(formatKey(key(code, { meta: true, shift }), true), code).toBe(name);
      // The other platform's modifier is the system's own (Ctrl+B moves the caret on a Mac).
      expect(formatKey(key(code, { meta: true, shift }), false), code).toBeNull();
      expect(formatKey(key(code, { ctrl: true, shift }), true), code).toBeNull();
      expect(formatKey(key(code, { ctrl: true, meta: true, shift }), false), code).toBeNull();
      // Never with Alt.
      expect(formatKey(key(code, { ctrl: true, alt: true, shift }), false), code).toBeNull();
    }
    // Shift only goes with the lists; the lists only with Shift.
    expect(formatKey(key("KeyB", { ctrl: true, shift: true }), false)).toBeNull();
    expect(formatKey(key("Digit8", { ctrl: true }), false)).toBeNull();
    expect(formatKey(key("KeyB", {}), false)).toBeNull();
  });

  // The tooltips, the names for assistive technology and the keys are one table.
  it("each_buttons_hint_is_its_key", () => {
    for (const b of FORMAT_BUTTONS) {
      const shifted = b.hint.includes("Shift+");
      const last = b.hint.slice(-1);
      const code = /\d/.test(last) ? `Digit${last}` : last === "\\" ? "Backslash" : `Key${last}`;
      expect(formatKey(key(code, { ctrl: true, shift: shifted }), false), b.label).toBe(b.key);
      expect(b.keys, b.label).toBe(`Control+${shifted ? "Shift+" : ""}${last}`);
      expect(b.hint, b.label).toBe(`Ctrl+${shifted ? "Shift+" : ""}${last}`);
    }
  });
});
