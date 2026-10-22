// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// The reply to an invitation (#884, C14b): Yes, No and Maybe in Google's order, sent at once; an
// occurrence asks This event or All events first, with focus on Back (plan Q5); one reply at a
// time, with focus kept on the button pressed; what it came to said in its line, and as a notice
// when it isn't a success or the control has gone (C14 critique C2-M1); the reply shown giving way
// when the row reloads (C1-L5); and the note, where PM knows the one Google holds.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const writes = vi.hoisted(() => ({ noteReply: vi.fn() }));
vi.mock("./useEventWrites", () => writes);

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import type { Reply } from "../../../lib/calendarEdit/guests";
import type { Rsvp, RsvpScope, WriteOutcome } from "../../../lib/types";
import { RsvpControl } from "./RsvpControl";

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

const SAVED: WriteOutcome = { outcome: "saved", warnings: [], left_behind: [] };

type OnReply = (reply: Rsvp, note: string | null, scope: RsvpScope | null) => Promise<WriteOutcome>;

/** A reply sent: each answered when the test says. */
function later() {
  const answers: ((o: WriteOutcome) => void)[] = [];
  const onReply = vi.fn<OnReply>(() => new Promise<WriteOutcome>((r) => answers.push(r)));
  return { onReply, answers };
}

interface Shown {
  current?: Reply | null;
  repeating?: boolean;
  forWhom?: string | null;
  note?: string | null;
  disabled?: boolean;
  onReply?: OnReply;
}

function control(over: Shown = {}) {
  const onReply = over.onReply ?? vi.fn<OnReply>(() => Promise.resolve(SAVED));
  const props = {
    current: null,
    repeating: false,
    forWhom: null,
    note: null,
    title: "Standup",
    ...over,
    onReply,
  };
  const view = render(<RsvpControl {...props} />);
  return { onReply, view, props };
}

const button = (name: string) => screen.getByRole("button", { name });
const pressed = () =>
  ["Yes", "No", "Maybe"].filter((n) => button(n).getAttribute("aria-pressed") === "true");
const status = () => screen.getByRole("status");

describe("RsvpControl", () => {
  it("shows Yes, No and Maybe in Google's order, the current one pressed, none before a reply", () => {
    control();
    const group = screen.getByRole("group", { name: "Going?" });
    expect(
      within(group)
        .getAllByRole("button")
        .map((b) => b.textContent),
    ).toEqual(["Yes", "No", "Maybe"]);
    expect(pressed()).toEqual([]);
    cleanup();
    control({ current: "awaiting" });
    expect(pressed()).toEqual([]);
    cleanup();
    control({ current: "maybe" });
    expect(pressed()).toEqual(["Maybe"]);
  });

  it("a one-off reply is sent at once and says so", async () => {
    const { onReply } = control({ current: "awaiting" });
    await act(async () => fireEvent.click(button("No")));
    expect(onReply).toHaveBeenCalledTimes(1);
    expect(onReply).toHaveBeenCalledWith("no", null, null);
    expect(status().textContent).toBe("You replied No.");
    expect(pressed()).toEqual(["No"]);
    // A success is said here only.
    expect(writes.noteReply).not.toHaveBeenCalled();
  });

  it("choosing the reply already shown sends nothing", () => {
    const { onReply } = control({ current: "yes" });
    fireEvent.click(button("Yes"));
    expect(onReply).not.toHaveBeenCalled();
    expect(screen.queryByRole("group", { name: /which occurrences/ })).toBeNull();
  });

  it("a repeating reply asks which occurrences first, with focus on Back", async () => {
    const parent = vi.fn();
    window.addEventListener("keydown", parent);
    try {
      const { onReply } = control({ current: "no", repeating: true });
      button("Yes").focus();
      fireEvent.click(button("Yes"));
      const step = screen.getByRole("group", { name: "Reply Yes to which occurrences?" });
      expect(
        within(step)
          .getAllByRole("button")
          .map((b) => b.textContent),
      ).toEqual(["This event", "All events", "Back"]);
      // Enter can't choose: it's on Back (plan Q5, C14 critique C2-M3).
      expect(document.activeElement).toBe(button("Back"));
      expect(onReply).not.toHaveBeenCalled();
      // Back returns to the reply buttons, focus on the one chosen.
      fireEvent.click(button("Back"));
      expect(document.activeElement).toBe(button("Yes"));
      expect(pressed()).toEqual(["No"]);
      // Escape does too, and stops there: the popover or the editor doesn't close.
      fireEvent.click(button("Maybe"));
      fireEvent.keyDown(button("Back"), { key: "Escape" });
      expect(document.activeElement).toBe(button("Maybe"));
      expect(parent).not.toHaveBeenCalled();
      // All events sends the scope, then focus goes back to the reply chosen.
      button("Yes").focus();
      fireEvent.click(button("Yes"));
      await act(async () => fireEvent.click(button("All events")));
      expect(onReply).toHaveBeenCalledWith("yes", null, "all");
      expect(status().textContent).toBe("You replied Yes for all events.");
      expect(screen.queryByRole("group", { name: /which occurrences/ })).toBeNull();
      expect(document.activeElement).toBe(button("Yes"));
      expect(pressed()).toEqual(["Yes"]);
      fireEvent.click(button("Maybe"));
      await act(async () => fireEvent.click(button("This event")));
      expect(onReply).toHaveBeenLastCalledWith("maybe", null, "this");
    } finally {
      window.removeEventListener("keydown", parent);
    }
  });

  it("nothing can be sent twice while one is on its way, and focus stays put", async () => {
    const { onReply, answers } = later();
    control({ current: "yes", onReply });
    button("No").focus();
    fireEvent.click(button("No"));
    expect(status().textContent).toBe("Sending your reply…");
    // Unavailable, never disabled: the button pressed keeps focus.
    for (const name of ["Yes", "No", "Maybe"]) {
      expect(button(name).getAttribute("aria-disabled")).toBe("true");
      expect((button(name) as HTMLButtonElement).disabled).toBe(false);
    }
    expect(document.activeElement).toBe(button("No"));
    fireEvent.click(button("Maybe"));
    fireEvent.click(button("No"));
    expect(onReply).toHaveBeenCalledTimes(1);
    await act(async () => answers[0](SAVED));
    expect(button("No").getAttribute("aria-disabled")).toBeNull();
    expect(document.activeElement).toBe(button("No"));
  });

  it("an occurrence's question sends once, and keeps Back and Escape while it's on its way", async () => {
    const parent = vi.fn();
    window.addEventListener("keydown", parent);
    try {
      const { onReply, answers } = later();
      control({ current: "yes", repeating: true, onReply });
      fireEvent.click(button("No"));
      fireEvent.click(button("All events"));
      fireEvent.click(button("All events"));
      fireEvent.click(button("This event"));
      expect(onReply).toHaveBeenCalledTimes(1);
      expect(button("Back").getAttribute("aria-disabled")).toBe("true");
      // Back does nothing; Escape goes on up (closing the popover then leaves a notice).
      fireEvent.click(button("Back"));
      expect(screen.getByRole("group", { name: "Reply No to which occurrences?" })).toBeTruthy();
      fireEvent.keyDown(button("Back"), { key: "Escape" });
      expect(parent).toHaveBeenCalledTimes(1);
      expect(screen.getByRole("group", { name: "Reply No to which occurrences?" })).toBeTruthy();
      await act(async () => answers[0](SAVED));
      expect(pressed()).toEqual(["No"]);
    } finally {
      window.removeEventListener("keydown", parent);
    }
  });

  it("a failed reply says why, keeps the old one pressed, and leaves a notice", async () => {
    const busy: WriteOutcome = { outcome: "busy" };
    const { onReply } = control({
      current: "yes",
      onReply: vi.fn<OnReply>(() => Promise.resolve(busy)),
    });
    await act(async () => fireEvent.click(button("No")));
    expect(onReply).toHaveBeenCalledTimes(1);
    expect(status().textContent).toBe("Google is busy right now. Reply again in a moment.");
    expect(status().className).toContain("text-st-look");
    expect(pressed()).toEqual(["Yes"]);
    expect(writes.noteReply).toHaveBeenCalledWith("Standup", busy, "no", null);
    // A refusal's tone is an error's.
    cleanup();
    const refused: WriteOutcome = { outcome: "read_only", reason: "not_invited" };
    control({ onReply: vi.fn<OnReply>(() => Promise.resolve(refused)) });
    await act(async () => fireEvent.click(button("Maybe")));
    expect(status().className).toContain("text-st-due");
    // A reply that threw is a failure, said with its words.
    cleanup();
    control({ onReply: vi.fn<OnReply>(() => Promise.reject(new Error("IPC went away."))) });
    await act(async () => fireEvent.click(button("Yes")));
    expect(status().textContent).toBe("IPC went away.");
    expect(pressed()).toEqual([]);
  });

  it("a needs-choice answer opens the occurrence question, without a notice", async () => {
    const needs: WriteOutcome = { outcome: "needs_choice", scope: true };
    const onReply = vi.fn<OnReply>().mockResolvedValueOnce(needs).mockResolvedValueOnce(SAVED);
    control({ current: "yes", onReply });
    button("No").focus();
    await act(async () => fireEvent.click(button("No")));
    expect(screen.getByRole("group", { name: "Reply No to which occurrences?" })).toBeTruthy();
    expect(document.activeElement).toBe(button("Back"));
    expect(status().textContent).toBe("Choose this occurrence or all of them, then reply again.");
    expect(writes.noteReply).not.toHaveBeenCalled();
    await act(async () => fireEvent.click(button("This event")));
    expect(onReply).toHaveBeenLastCalledWith("no", null, "this");
    expect(status().textContent).toBe("You replied No.");
  });

  // An answer that comes after the user has moved on (Tab out to the grid) never takes focus back.
  it("never takes focus from outside the control when an answer comes", async () => {
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    try {
      const needs: WriteOutcome = { outcome: "needs_choice", scope: true };
      const first = later();
      control({ current: "yes", onReply: first.onReply });
      fireEvent.click(button("No"));
      outside.focus();
      await act(async () => first.answers[0](needs));
      expect(screen.getByRole("group", { name: "Reply No to which occurrences?" })).toBeTruthy();
      expect(document.activeElement).toBe(outside);
      // Sent from the question, then focus taken elsewhere: the answer leaves it there.
      fireEvent.click(button("All events"));
      outside.focus();
      await act(async () => first.answers[1](SAVED));
      expect(screen.queryByRole("group", { name: /which occurrences/ })).toBeNull();
      expect(document.activeElement).toBe(outside);
    } finally {
      outside.remove();
    }
  });

  // C14 critique C2-M1: the popover can close while the reply is on its way.
  it("a reply that fails after the popover closed shows a notice", async () => {
    const { onReply, answers } = later();
    const { view } = control({ current: "yes", onReply });
    fireEvent.click(button("No"));
    view.unmount();
    const failed: WriteOutcome = { outcome: "unconfirmed" };
    await act(async () => answers[0](failed));
    expect(writes.noteReply).toHaveBeenCalledTimes(1);
    expect(writes.noteReply).toHaveBeenCalledWith("Standup", failed, "no", null);
    // And one that went: said there too, since its line has gone.
    const second = later();
    const again = control({ current: "yes", repeating: true, onReply: second.onReply });
    fireEvent.click(button("Maybe"));
    fireEvent.click(button("All events"));
    again.view.unmount();
    await act(async () => second.answers[0](SAVED));
    expect(writes.noteReply).toHaveBeenLastCalledWith("Standup", SAVED, "maybe", "all");
  });

  // C14 critique C1-L5: a reload with Google's copy, or a change made in Google, wins.
  it("the reply shown gives way when the row reloads", async () => {
    const { view, props } = control({ current: "awaiting" });
    await act(async () => fireEvent.click(button("Yes")));
    expect(pressed()).toEqual(["Yes"]);
    // A reload that hasn't caught up yet keeps what was sent.
    view.rerender(<RsvpControl {...props} current="awaiting" />);
    expect(pressed()).toEqual(["Yes"]);
    view.rerender(<RsvpControl {...props} current="no" />);
    expect(pressed()).toEqual(["No"]);
  });

  // PR B review round 1: once the row has moved on, a change in Google back to the answer the reply
  // was sent over shows, never the reply sent.
  it("the reply shown gives way for good once the row moves on", async () => {
    const { view, props } = control({ current: "yes" });
    await act(async () => fireEvent.click(button("No")));
    expect(pressed()).toEqual(["No"]);
    view.rerender(<RsvpControl {...props} current="no" />);
    expect(pressed()).toEqual(["No"]);
    // Answered Yes again in Google Calendar, say.
    view.rerender(<RsvpControl {...props} current="yes" />);
    expect(pressed()).toEqual(["Yes"]);
  });

  // PR B review round 1: Google kept this occurrence's own answer when the reply went to all events
  // (its day among those the outcome names), so the reply sent isn't shown as its own.
  it("an all-events reply some occurrence kept its own answer against leaves it to the row", async () => {
    const kept: WriteOutcome = {
      outcome: "saved",
      warnings: ["kept_own_reply"],
      left_behind: ["2026-10-19"],
    };
    const { view, props } = control({
      current: "no",
      repeating: true,
      onReply: vi.fn<OnReply>(() => Promise.resolve(kept)),
    });
    fireEvent.click(button("Yes"));
    await act(async () => fireEvent.click(button("All events")));
    expect(status().textContent).toBe(
      "You replied Yes for all events. 1 occurrence you answered on its own kept its answer.",
    );
    expect(pressed()).toEqual(["No"]);
    // So Yes can still go to this occurrence alone.
    fireEvent.click(button("Yes"));
    expect(screen.getByRole("group", { name: "Reply Yes to which occurrences?" })).toBeTruthy();
    fireEvent.click(button("Back"));
    // Where this one took it, its row (or the editor's copy) says so.
    view.rerender(<RsvpControl {...props} current="yes" />);
    expect(pressed()).toEqual(["Yes"]);
  });

  // PR B review round 2: an all-events reply PM couldn't read back yet may not have reached this
  // occurrence (answered Yes on its own), so the reply sent isn't shown as its own either.
  it("an all-events reply not read back yet leaves the occurrence's answer to the row", async () => {
    const pending: WriteOutcome = {
      outcome: "saved",
      warnings: ["mirror_refresh_pending"],
      left_behind: [],
    };
    const { view, props } = control({
      current: "yes",
      repeating: true,
      onReply: vi.fn<OnReply>(() => Promise.resolve(pending)),
    });
    fireEvent.click(button("Maybe"));
    await act(async () => fireEvent.click(button("All events")));
    expect(status().textContent).toBe(
      "You replied Maybe for all events. PM's calendar will show it after the next refresh.",
    );
    expect(pressed()).toEqual(["Yes"]);
    // Google kept its Yes: the row lands unchanged, and Maybe can still go to it alone.
    view.rerender(<RsvpControl {...props} current="yes" />);
    expect(pressed()).toEqual(["Yes"]);
    fireEvent.click(button("Maybe"));
    expect(screen.getByRole("group", { name: "Reply Maybe to which occurrences?" })).toBeTruthy();
    // To this occurrence alone it went to this copy: shown, though not read back yet.
    await act(async () => fireEvent.click(button("This event")));
    expect(pressed()).toEqual(["Maybe"]);
  });

  it("replying for a shared calendar names it", () => {
    control({ forWhom: "Team" });
    expect(screen.getByRole("group", { name: "Going? Replying for Team." })).toBeTruthy();
  });

  it("disabled, nothing can be pressed", () => {
    const { onReply } = control({ disabled: true, note: "" });
    for (const name of ["Yes", "No", "Maybe", "Add a note"]) {
      expect((button(name) as HTMLButtonElement).disabled).toBe(true);
    }
    fireEvent.click(button("No"));
    expect(onReply).not.toHaveBeenCalled();
  });

  describe("the note", () => {
    const box = () => screen.getByLabelText<HTMLTextAreaElement>("Note");
    const type = (text: string) => fireEvent.change(box(), { target: { value: text } });

    it("no note box without a known note", () => {
      control({ current: "yes" });
      expect(screen.queryByRole("button", { name: "Add a note" })).toBeNull();
      expect(screen.queryByLabelText("Note")).toBeNull();
    });

    it("a note waits for a reply", () => {
      const { onReply } = control({ note: "" });
      fireEvent.click(button("Add a note"));
      type("running late");
      expect(box().getAttribute("aria-describedby")).toBeTruthy();
      const hint = document.getElementById(box().getAttribute("aria-describedby")!);
      expect(hint?.textContent).toContain("Everyone invited can see it.");
      expect(hint?.textContent).toContain("Choose Yes, No or Maybe first.");
      const send = button("Send note") as HTMLButtonElement;
      expect(send.disabled).toBe(true);
      fireEvent.click(send);
      expect(onReply).not.toHaveBeenCalled();
    });

    it("Send note sends the reply shown with the text", async () => {
      const { onReply } = control({ current: "maybe", note: "" });
      fireEvent.click(button("Add a note"));
      expect(box().value).toBe("");
      type("running late");
      await act(async () => fireEvent.click(button("Send note")));
      expect(onReply).toHaveBeenCalledWith("maybe", "running late", null);
      expect(status().textContent).toBe("You replied Maybe.");
    });

    it("emptying an existing note offers Remove note and sends an empty note", async () => {
      const { onReply, view, props } = control({ current: "yes", note: "late" });
      fireEvent.click(button("Change note"));
      expect(box().value).toBe("late");
      type("  ");
      await act(async () => fireEvent.click(button("Remove note")));
      expect(onReply).toHaveBeenCalledWith("yes", "  ", null);
      // Sent: the note held is none now, so the box offers Send note again.
      expect(screen.getByRole("button", { name: "Send note" })).toBeTruthy();
      // And once the editor reads Google's copy again, its note is what shows.
      view.rerender(<RsvpControl {...props} note="from Google" />);
      type("");
      expect(screen.getByRole("button", { name: "Remove note" })).toBeTruthy();
    });

    // PR B review round 1: as the reply sent, the note sent gives way for good once `note` moves on.
    it("the note sent gives way for good once the note held moves on", async () => {
      const { view, props } = control({ current: "yes", note: "" });
      fireEvent.click(button("Add a note"));
      type("late");
      await act(async () => fireEvent.click(button("Send note")));
      type("");
      expect(screen.getByRole("button", { name: "Remove note" })).toBeTruthy();
      view.rerender(<RsvpControl {...props} note="late" />);
      expect(screen.getByRole("button", { name: "Remove note" })).toBeTruthy();
      // Removed again in Google: no note held, so nothing to remove.
      view.rerender(<RsvpControl {...props} note="" />);
      expect(screen.getByRole("button", { name: "Send note" })).toBeTruthy();
    });

    // PR B review round 1: Send note is only unavailable while a reply from the occurrence question
    // is on its way, so it must send nothing and leave that question as it is.
    it("Send note while an occurrence's reply is on its way sends nothing", async () => {
      const { onReply, answers } = later();
      control({ current: "yes", repeating: true, note: "", onReply });
      fireEvent.click(button("Add a note"));
      type("late");
      fireEvent.click(button("No"));
      fireEvent.click(button("This event"));
      expect(button("Send note").getAttribute("aria-disabled")).toBe("true");
      fireEvent.click(button("Send note"));
      expect(screen.getByRole("group", { name: "Reply No to which occurrences?" })).toBeTruthy();
      await act(async () => answers[0](SAVED));
      expect(onReply).toHaveBeenCalledTimes(1);
      expect(onReply).toHaveBeenCalledWith("no", null, "this");
      expect(screen.queryByRole("group", { name: /which occurrences/ })).toBeNull();
      // The note is still there to send.
      expect(box().value).toBe("late");
    });

    it("a note over 1,000 characters can't be sent", () => {
      const { onReply } = control({ current: "yes", note: "" });
      fireEvent.click(button("Add a note"));
      type("é".repeat(1000));
      expect((button("Send note") as HTMLButtonElement).disabled).toBe(false);
      type("é".repeat(1001));
      expect(screen.getByText("A note can be up to 1,000 characters.")).toBeTruthy();
      expect(box().getAttribute("aria-invalid")).toBe("true");
      const send = button("Send note") as HTMLButtonElement;
      expect(send.disabled).toBe(true);
      fireEvent.click(send);
      expect(onReply).not.toHaveBeenCalled();
    });

    it("on an occurrence, asks which ones first and sends the note with the scope", async () => {
      const { onReply } = control({ current: "yes", repeating: true, note: "" });
      fireEvent.click(button("Add a note"));
      type("late");
      fireEvent.click(button("Send note"));
      expect(onReply).not.toHaveBeenCalled();
      expect(screen.getByRole("group", { name: "Reply Yes to which occurrences?" })).toBeTruthy();
      await act(async () => fireEvent.click(button("This event")));
      expect(onReply).toHaveBeenCalledWith("yes", "late", "this");
    });
  });
});
