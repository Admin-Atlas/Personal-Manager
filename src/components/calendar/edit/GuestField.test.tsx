// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// The editor's guest list (#884, C12): adding by every way Google takes (Enter, a comma, Tab, leaving
// the box, a paste, a suggestion), refusing what isn't an address, the keyboard of a combobox whose
// Escape is its own, rows the user can and can't change, and the three guest permissions. Driven
// through a harness that keeps the list, as the editor will.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import type { GuestDraft, GuestPerms } from "../../../lib/calendarEdit/eventDraft";
import type { GuestSuggestion } from "../../../lib/calendarEdit/guests";
import { GuestField } from "./GuestField";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const guest = (email: string, over: Partial<GuestDraft> = {}): GuestDraft => ({
  email,
  name: null,
  response: null,
  optional: false,
  organizer: false,
  self: false,
  room: false,
  ...over,
});
const ME = guest("me@x.org", { name: "Me", organizer: true, self: true, response: "accepted" });
const ANN = guest("ann@x.org", { name: "Ann", response: "declined" });
const PERMS: GuestPerms = { modify: false, invite_others: true, see_others: true };
const SUGGESTED: GuestSuggestion[] = [
  { email: "ann@x.org", name: "Ann", count: 4 },
  { email: "andy@x.org", name: "Andy", count: 3 },
  { email: "me@x.org", name: "Me", count: 2 },
  { email: "dana@x.org", name: null, count: 1 },
];

interface Opts {
  guests?: GuestDraft[];
  perms?: GuestPerms;
  disabled?: boolean;
  omitted?: boolean;
  me?: string | null;
  organizer?: string | null;
}

function setUp({
  guests = [ME, ANN],
  perms = PERMS,
  disabled = false,
  omitted = false,
  me = "me@x.org",
  organizer = "me@x.org",
}: Opts = {}) {
  const onChange = vi.fn<(g: GuestDraft[]) => void>();
  const onPerms = vi.fn<(p: GuestPerms) => void>();
  const onReject = vi.fn<(t: string | null) => void>();
  const latest = { guests, perms };
  // The editor's base: Google's list as opened, until a rebase hands it a newer one.
  const editor: { rebase: (base: GuestDraft[]) => void } = { rebase: () => {} };
  function Harness() {
    const [list, setList] = useState(guests);
    const [base, setBase] = useState<GuestDraft[]>(guests);
    const [p, setP] = useState(perms);
    editor.rebase = setBase;
    return (
      <GuestField
        guests={list}
        base={base}
        perms={p}
        suggestions={SUGGESTED}
        me={me}
        organizer={organizer}
        disabled={disabled}
        omitted={omitted}
        onChange={(g) => {
          onChange(g);
          latest.guests = g;
          setList(g);
        }}
        onPerms={(next) => {
          onPerms(next);
          latest.perms = next;
          setP(next);
        }}
        onReject={onReject}
      />
    );
  }
  render(<Harness />);
  const box = screen.getByRole("combobox", { name: "Add guests" }) as HTMLInputElement;
  const type = (text: string) => fireEvent.change(box, { target: { value: text } });
  const emails = () => latest.guests.map((g) => g.email);
  const rebase = (base: GuestDraft[]) => act(() => editor.rebase(base));
  return { box, type, emails, latest, onChange, onPerms, onReject, rebase };
}

const rows = () => within(screen.getByRole("list", { name: "Guests" })).getAllByRole("listitem");
const options = () => screen.queryAllByRole("option");

describe("GuestField, adding", () => {
  it("adds by Enter, a comma, a semicolon, Tab and leaving the box", () => {
    const { box, type, emails, onReject } = setUp({ guests: [ME] });
    type("a@x.org");
    fireEvent.keyDown(box, { key: "Enter" });
    type("b@x.org");
    expect(fireEvent.keyDown(box, { key: "," })).toBe(false); // the comma isn't typed
    type("c@x.org");
    fireEvent.keyDown(box, { key: ";" });
    type("d@x.org");
    fireEvent.keyDown(box, { key: "Tab" });
    type(" e@x.org ");
    fireEvent.blur(box);
    expect(emails()).toEqual(["me@x.org", "a@x.org", "b@x.org", "c@x.org", "d@x.org", "e@x.org"]);
    expect(box.value).toBe("");
    // Each new guest is a plain one, with no reply yet.
    expect(rows()[1].textContent).toContain("a@x.org");
    expect(onReject).not.toHaveBeenCalledWith(expect.any(String));
  });

  it("never lets Enter reach the editor, even with nothing to add", () => {
    const { box } = setUp();
    expect(fireEvent.keyDown(box, { key: "Enter" })).toBe(false);
  });

  it("refuses what isn't an address, and says so until it's cleared or added", () => {
    const { box, type, emails, onReject } = setUp({ guests: [ME] });
    type("bob at x");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(screen.getByText("“bob at x” isn't an email address.")).toBeTruthy();
    expect(box.getAttribute("aria-invalid")).toBe("true");
    expect(onReject).toHaveBeenLastCalledWith("bob at x");
    // A comma after it is typed, not taken as an add.
    expect(fireEvent.keyDown(box, { key: "," })).toBe(true);
    fireEvent.blur(box);
    expect(onReject).toHaveBeenCalledTimes(1);
    expect(box.value).toBe("bob at x");
    expect(emails()).toEqual(["me@x.org"]);
    // Cleared: nothing refused any more.
    type("");
    expect(onReject).toHaveBeenLastCalledWith(null);
    expect(screen.queryByText(/isn't an email address/)).toBeNull();
    // Refused again, then fixed and added.
    type("bob@");
    fireEvent.blur(box);
    expect(onReject).toHaveBeenLastCalledWith("bob@");
    type("bob@x.org");
    fireEvent.blur(box);
    expect(onReject).toHaveBeenLastCalledWith(null);
    expect(emails()).toEqual(["me@x.org", "bob@x.org"]);
  });

  it("adds every address pasted at once, leaving the rest in the box", () => {
    const { box, emails } = setUp({ guests: [ME] });
    fireEvent.paste(box, {
      clipboardData: { getData: () => "a@x.org, Cy Young <c@x.org>; e@x.org" },
    });
    expect(emails()).toEqual(["me@x.org", "a@x.org", "c@x.org", "e@x.org"]);
    expect(box.value).toBe("");
    fireEvent.paste(box, { clipboardData: { getData: () => "f@x.org and more" } });
    expect(emails()).toContain("f@x.org");
    expect(box.value).toBe("and more");
  });

  it("adds no one twice, and says so for a few seconds", () => {
    vi.useFakeTimers();
    const { box, type, emails } = setUp();
    type("ANN@x.org");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(emails()).toEqual(["me@x.org", "ann@x.org"]);
    expect(screen.getByRole("status").textContent).toBe("Already on the list.");
    expect(box.value).toBe("");
    act(() => {
      vi.advanceTimersByTime(4000);
    });
    expect(screen.getByRole("status").textContent).toBe("");
  });

  it("never adds the organiser, on Google's list or not, and says why", () => {
    vi.useFakeTimers();
    // An event the user organises with no guests: Google sends no list, only the organiser.
    const { box, type, emails, onChange, onReject } = setUp({ guests: [] });
    type("Me@x.org");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("status").textContent).toBe(
      "That's the organiser, who is on it already.",
    );
    expect(box.value).toBe("");
    expect(onReject).not.toHaveBeenCalledWith(expect.any(String));
    // A pasted "To:" line with the organiser in it adds everyone else.
    fireEvent.paste(box, { clipboardData: { getData: () => "me@x.org, ann@x.org" } });
    expect(emails()).toEqual(["ann@x.org"]);
  });

  it("leaves an input method's keys to it", () => {
    const { box, type, emails, onReject } = setUp({ guests: [ME] });
    type("an");
    expect(box.getAttribute("aria-expanded")).toBe("true");
    fireEvent.keyDown(box, { key: "ArrowDown", keyCode: 229 });
    fireEvent.keyDown(box, { key: "ArrowDown", isComposing: true });
    expect(box.getAttribute("aria-activedescendant")).toBeNull();
    // The Enter that confirms a composed name neither adds nor refuses it.
    type("王");
    fireEvent.keyDown(box, { key: "Enter", isComposing: true });
    fireEvent.keyDown(box, { key: "Enter", keyCode: 229 });
    expect(emails()).toEqual(["me@x.org"]);
    expect(onReject).not.toHaveBeenCalled();
    expect(screen.queryByText(/isn't an email address/)).toBeNull();
  });

  it("wraps a long refused text rather than pushing the editor sideways", () => {
    const { box, type } = setUp({ guests: [ME] });
    type("https://meet.example.com/abc-defg-hij?authuser=0&hs=179");
    fireEvent.blur(box);
    expect(screen.getByText(/isn't an email address/).className).toContain("break-words");
  });

  it("knows a room by its address", () => {
    const { box, type, latest } = setUp({ guests: [ME] });
    type("room1@resource.calendar.google.com");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(latest.guests[1].room).toBe(true);
    expect(rows()[1].textContent).toContain("Room");
  });
});

describe("GuestField, suggestions", () => {
  it("offers people by name or address, never anyone already on it", () => {
    const { type } = setUp();
    type("a");
    const offered = options().map((o) => o.textContent);
    expect(offered).toEqual(["Andyandy@x.org", "dana@x.org"]);
  });

  it("never offers you or the organiser, even on a list neither is on", () => {
    const { type } = setUp({ guests: [ANN], me: "ME@x.org", organizer: null });
    type("me");
    expect(options()).toHaveLength(0);
    cleanup();
    const organiser = setUp({ guests: [ANN], me: null, organizer: "Me@X.org" });
    organiser.type("me");
    expect(options()).toHaveLength(0);
    cleanup();
    // Without an account or an organiser to leave out, the same address is offered like anyone.
    const other = setUp({ guests: [ANN], me: null, organizer: null });
    other.type("me");
    expect(options().map((o) => o.textContent)).toEqual(["Meme@x.org"]);
  });

  it("starts with none active, so Enter adds what was typed", () => {
    const { box, type, emails } = setUp({ guests: [ME] });
    type("andy@x.org");
    expect(options()).toHaveLength(1);
    expect(box.getAttribute("aria-activedescendant")).toBeNull();
    fireEvent.keyDown(box, { key: "Enter" });
    expect(emails()).toEqual(["me@x.org", "andy@x.org"]);
  });

  it("moves with the arrow keys, wrapping, and Enter adds the one active", () => {
    const { box, type, latest } = setUp({ guests: [ME] });
    type("a");
    expect(box.getAttribute("aria-expanded")).toBe("true");
    // Ann, Andy and dana, most seen first; you aren't offered.
    expect(options()).toHaveLength(3);
    const activeIs = (i: number) => {
      expect(box.getAttribute("aria-activedescendant")).toBe(options()[i].id);
      expect(options().map((o) => o.getAttribute("aria-selected"))).toEqual(
        [0, 1, 2].map((j) => String(j === i)),
      );
    };
    fireEvent.keyDown(box, { key: "ArrowDown" });
    activeIs(0);
    fireEvent.keyDown(box, { key: "ArrowDown" });
    fireEvent.keyDown(box, { key: "ArrowDown" });
    activeIs(2);
    fireEvent.keyDown(box, { key: "ArrowDown" });
    activeIs(0);
    fireEvent.keyDown(box, { key: "ArrowUp" });
    activeIs(2);
    fireEvent.keyDown(box, { key: "ArrowUp" });
    fireEvent.keyDown(box, { key: "ArrowUp" });
    activeIs(0);
    fireEvent.keyDown(box, { key: "Enter" });
    expect(latest.guests[1]).toMatchObject({ email: "ann@x.org", name: "Ann", response: null });
    expect(box.value).toBe("");
    expect(box.getAttribute("aria-expanded")).toBe("false");
  });

  it("adds one pressed with the pointer, keeping focus in the box", () => {
    const { box, type, emails } = setUp({ guests: [ME] });
    box.focus();
    type("dan");
    const pressed = fireEvent.mouseDown(options()[0]);
    expect(pressed).toBe(false);
    expect(emails()).toEqual(["me@x.org", "dana@x.org"]);
  });

  it("keeps Escape its own while it has something to close or clear", () => {
    const { box, type } = setUp();
    const editor = vi.fn();
    window.addEventListener("keydown", editor);
    try {
      type("an");
      expect(options().length).toBeGreaterThan(0);
      fireEvent.keyDown(box, { key: "Escape" });
      expect(box.getAttribute("aria-expanded")).toBe("false");
      expect(box.value).toBe("an");
      expect(editor).not.toHaveBeenCalled();
      // Closed, with text: it clears the text.
      fireEvent.keyDown(box, { key: "Escape" });
      expect(box.value).toBe("");
      expect(editor).not.toHaveBeenCalled();
      // Empty: the editor's.
      fireEvent.keyDown(box, { key: "Escape" });
      expect(editor).toHaveBeenCalledTimes(1);
      // ArrowDown opens them again.
      type("an");
      fireEvent.keyDown(box, { key: "Escape" });
      fireEvent.keyDown(box, { key: "ArrowDown" });
      expect(box.getAttribute("aria-expanded")).toBe("true");
    } finally {
      window.removeEventListener("keydown", editor);
    }
  });
});

describe("GuestField, the list", () => {
  it("shows each guest's reply, name and address, and sums them up", () => {
    setUp({ guests: [ME, ANN, guest("bo@x.org", { response: "needsAction", optional: true })] });
    expect(screen.getByText("3 guests: 1 yes, 1 no, 1 awaiting")).toBeTruthy();
    const [me, ann] = rows();
    expect(me.textContent).toContain("Organiser");
    expect(within(ann).getByText("declined").className).toContain("sr-only");
    expect(within(ann).getByText("ann@x.org")).toBeTruthy();
    expect(within(ann).getByText("Ann").getAttribute("title")).toBe("ann@x.org");
  });

  it("gives the organiser's and the user's rows no Optional and no remove", () => {
    const organiser = guest("o@x.org", { organizer: true });
    const you = guest("you@x.org", { self: true });
    setUp({ guests: [organiser, you, ANN] });
    const [o, y, a] = rows();
    for (const row of [o, y]) expect(within(row).queryAllByRole("button")).toHaveLength(0);
    expect(within(a).getAllByRole("button")).toHaveLength(2);
  });

  it("knows the organiser by address too, on a list Google sent without its marks", () => {
    // The save never sends a change to the organiser's entry, so the row offers none.
    setUp({ guests: [guest("O@x.org", { name: "Olga" }), ANN], organizer: "o@x.org" });
    const [o, a] = rows();
    expect(within(o).queryAllByRole("button")).toHaveLength(0);
    expect(o.textContent).toContain("Organiser");
    expect(within(a).getAllByRole("button")).toHaveLength(2);
  });

  it("marks a guest optional and back, under one name", () => {
    const { latest } = setUp();
    const toggle = screen.getByRole("button", { name: "Optional, Ann" });
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(toggle);
    expect(latest.guests[1].optional).toBe(true);
    expect(screen.getByRole("button", { name: "Optional, Ann" }).getAttribute("aria-pressed")).toBe(
      "true",
    );
    fireEvent.click(screen.getByRole("button", { name: "Optional, Ann" }));
    expect(latest.guests[1].optional).toBe(false);
  });

  it("shows who is optional by more than aria-pressed: filled when on, an --ink4 edge when off", () => {
    setUp();
    const toggle = () => screen.getByRole("button", { name: "Optional, Ann" });
    expect(toggle().classList).toContain("shadow-[inset_0_0_0_1px_var(--ink4)]");
    expect(toggle().classList).not.toContain("bg-accent");
    fireEvent.click(toggle());
    expect(toggle().classList).toContain("bg-accent");
    expect(toggle().classList).toContain("text-accent-ink");
    expect(toggle().className).not.toContain("border2");
  });

  it("takes a guest off, and one added back is Google's entry, reply and all, not the one taken off", () => {
    const { box, type, latest } = setUp();
    // Made optional, then taken off: added back, Ann is as Google has her (required, declined), so
    // the save sends nothing for her.
    fireEvent.click(screen.getByRole("button", { name: "Optional, Ann" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove Ann" }));
    expect(latest.guests.map((g) => g.email)).toEqual(["me@x.org"]);
    type("Ann@X.org");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(latest.guests[1]).toEqual(ANN);
  });

  it("after a rebase, adds back Google's entry as it is now, or a new guest", () => {
    const { box, type, latest, rebase } = setUp();
    fireEvent.click(screen.getByRole("button", { name: "Remove Ann" }));
    // Google's copy now: Ann has accepted since.
    rebase([ME, { ...ANN, response: "accepted" }]);
    type("ann@x.org");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(latest.guests[1]).toEqual({ ...ANN, response: "accepted" });
    fireEvent.click(screen.getByRole("button", { name: "Remove Ann" }));
    // Google took her off too: she's a new guest, with no reply.
    rebase([ME]);
    type("ann@x.org");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(latest.guests[1]).toEqual(guest("ann@x.org"));
  });

  it("keeps focus in the field when a guest is taken off", () => {
    const BO = guest("bo@x.org");
    const CY = guest("cy@x.org");
    setUp({ guests: [ME, ANN, BO, CY] });
    const button = (name: string) => screen.getByRole("button", { name });
    // The next row's ×, else the one before, else the box: never the page behind the dialog.
    button("Remove Ann").focus();
    fireEvent.click(button("Remove Ann"));
    expect(document.activeElement).toBe(button("Remove bo@x.org"));
    fireEvent.click(button("Remove cy@x.org"));
    expect(document.activeElement).toBe(button("Remove bo@x.org"));
    fireEvent.click(button("Remove bo@x.org"));
    expect(document.activeElement).toBe(screen.getByRole("combobox", { name: "Add guests" }));
  });

  it("checks and locks the other two permissions with Modify event, as Google does", () => {
    const { onPerms } = setUp();
    const modify = screen.getByLabelText("Modify event") as HTMLInputElement;
    const invite = screen.getByLabelText("Invite others") as HTMLInputElement;
    const see = screen.getByLabelText("See guest list") as HTMLInputElement;
    expect([modify.checked, invite.checked, see.checked]).toEqual([false, true, true]);
    fireEvent.click(see);
    expect(onPerms).toHaveBeenLastCalledWith({
      modify: false,
      invite_others: true,
      see_others: false,
    });
    fireEvent.click(modify);
    expect(onPerms).toHaveBeenLastCalledWith({
      modify: true,
      invite_others: true,
      see_others: true,
    });
    expect([invite.checked, see.checked]).toEqual([true, true]);
    expect([invite.disabled, see.disabled]).toEqual([true, true]);
    // Off again: the other two can change, and stay as they are.
    fireEvent.click(modify);
    expect(onPerms).toHaveBeenLastCalledWith({
      modify: false,
      invite_others: true,
      see_others: true,
    });
    expect(invite.disabled).toBe(false);
  });

  it("draws the other two checked under Modify event, whatever Google stored for them", () => {
    // Google's API (or a merge after a conflict) can hold Modify on with the others off.
    const { onPerms } = setUp({ perms: { modify: true, invite_others: false, see_others: false } });
    const invite = screen.getByLabelText("Invite others") as HTMLInputElement;
    const see = screen.getByLabelText("See guest list") as HTMLInputElement;
    expect([invite.checked, see.checked]).toEqual([true, true]);
    expect([invite.disabled, see.disabled]).toEqual([true, true]);
    // Off again: what Google stored shows, and can change.
    fireEvent.click(screen.getByLabelText("Modify event"));
    expect(onPerms).toHaveBeenLastCalledWith({
      modify: false,
      invite_others: false,
      see_others: false,
    });
    expect([invite.checked, invite.disabled]).toEqual([false, false]);
  });

  it("shows a list Google cut short, and changes none of it", () => {
    const { box } = setUp({ omitted: true });
    expect(
      screen.getByText("Google didn't send this event's whole guest list, so PM can't change it."),
    ).toBeTruthy();
    expect(box.disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Remove Ann" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect((screen.getByLabelText("Modify event") as HTMLInputElement).disabled).toBe(true);
  });

  it("dims the Optional toggle when it can't be pressed, on or off", () => {
    setUp({ omitted: true, guests: [ME, ANN, guest("bo@x.org", { optional: true })] });
    const off = screen.getByRole("button", { name: "Optional, Ann" }) as HTMLButtonElement;
    const on = screen.getByRole("button", { name: "Optional, bo@x.org" }) as HTMLButtonElement;
    expect([off.disabled, on.disabled]).toEqual([true, true]);
    // Its words dim as the × beside it does, and the fill gives way to the surface.
    expect(off.classList).toContain("disabled:text-ink4");
    expect(on.classList).toContain("disabled:text-ink4");
    expect(on.classList).toContain("disabled:bg-surface");
  });

  it("when off, still shows the list, and every control is off", () => {
    const { box, type } = setUp({ disabled: true });
    expect(rows()).toHaveLength(2);
    expect(box.disabled).toBe(true);
    for (const b of screen.getAllByRole("button"))
      expect((b as HTMLButtonElement).disabled).toBe(true);
    for (const c of screen.getAllByRole("checkbox"))
      expect((c as HTMLInputElement).disabled).toBe(true);
    type("an");
    expect(options()).toHaveLength(0);
    expect(screen.queryByText(/didn't send this event's whole guest list/)).toBeNull();
  });
});
