// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// The editor's video-call row (#884, C18): Google Calendar's words for adding and removing a Meet
// call, another service's call shown and kept, Join opening the link, focus following a press to
// the button that replaced it, and the call's name and link drawn as text.

import { useState } from "react";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const openUrl = vi.hoisted(() => vi.fn<(url: string) => Promise<void>>());
vi.mock("../../../lib/ipc", () => ({ openUrl: (url: string) => openUrl(url) }));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import type { CallForEdit } from "../../../lib/types";
import { JOIN_FAILED } from "../../../lib/calendarEdit/editReasons";
import { MeetField } from "./MeetField";

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

const LINK = "https://meet.google.com/abc-defg-hij";

const meet = (state: CallForEdit["state"] = "ready"): CallForEdit => ({
  kind: "meet",
  name: "Google Meet",
  join: state === "ready" ? LINK : null,
  state,
});

const zoom: CallForEdit = {
  kind: "add_on",
  name: "Zoom Meeting",
  join: "https://zoom.us/j/987",
  state: "ready",
};

interface Shown {
  call?: CallForEdit | null;
  value?: boolean;
  offered?: boolean;
  disabled?: boolean;
  onChange?: (meet: boolean) => void;
}

/** The row as the editor holds it: `value` follows `onChange`, as the draft does. */
function Row({ call = null, value = false, offered = true, disabled, onChange }: Shown) {
  const [meetOn, setMeetOn] = useState(value);
  return (
    <MeetField
      call={call}
      value={meetOn}
      offered={offered}
      disabled={disabled}
      onChange={(next) => {
        onChange?.(next);
        setMeetOn(next);
      }}
    />
  );
}

const group = () => screen.getByRole("group", { name: "Video call" });
const button = (name: string) => within(group()).getByRole("button", { name });
const noButton = (name: string | RegExp) =>
  expect(within(group()).queryByRole("button", { name })).toBeNull();

describe("MeetField", () => {
  it("offers Add Google Meet video conferencing where the calendar offers Meet, and nothing where it doesn't", () => {
    const onChange = vi.fn();
    render(<Row onChange={onChange} />);
    fireEvent.click(button("Add Google Meet video conferencing"));
    expect(onChange).toHaveBeenCalledWith(true);
    expect(
      within(group()).getByText(
        "Google Meet video conferencing. Google makes the link when you save.",
      ),
    ).toBeTruthy();
    cleanup();
    const { container } = render(<Row offered={false} />);
    expect(screen.queryByRole("group", { name: "Video call" })).toBeNull();
    expect(container.textContent).toBe("");
  });

  it("a ready call shows Join with Google Meet, its link and Remove", () => {
    render(<Row call={meet()} value />);
    expect(button("Join with Google Meet")).toBeTruthy();
    // The link as it reads, without its scheme, and a way to take it to another browser.
    expect(within(group()).getByText("meet.google.com/abc-defg-hij")).toBeTruthy();
    expect(button("Copy link to the call")).toBeTruthy();
    expect(button("Remove the video call").textContent).toBe("Remove");
    noButton(/^Add/);
  });

  it("Remove says it goes when you save, and Keep it brings it back", () => {
    const onChange = vi.fn();
    render(<Row call={meet()} value onChange={onChange} />);
    fireEvent.click(button("Remove the video call"));
    expect(onChange).toHaveBeenLastCalledWith(false);
    expect(within(group()).getByText("The video call will be removed when you save.")).toBeTruthy();
    noButton("Join with Google Meet");
    fireEvent.click(button("Keep it"));
    expect(onChange).toHaveBeenLastCalledWith(true);
    expect(button("Join with Google Meet")).toBeTruthy();
    expect(within(group()).queryByText(/will be removed/)).toBeNull();
  });

  it("a pending call says Google is still making it, and can still be removed", () => {
    const onChange = vi.fn();
    render(<Row call={meet("pending")} value onChange={onChange} />);
    expect(
      within(group()).getByText(
        "Google is still making the video call. Its link shows after a later refresh.",
      ),
    ).toBeTruthy();
    noButton("Join with Google Meet");
    fireEvent.click(button("Remove the video call"));
    expect(onChange).toHaveBeenCalledWith(false);
  });

  it("a failed request says so and offers Add again", () => {
    const onChange = vi.fn();
    render(<Row call={meet("failed")} onChange={onChange} />);
    expect(within(group()).getByText("Google couldn't make the video call.")).toBeTruthy();
    fireEvent.click(button("Add Google Meet video conferencing"));
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it("another service's call shows its name and Join, and no Add or Remove", () => {
    render(<Row call={zoom} />);
    expect(within(group()).getByText("Zoom Meeting")).toBeTruthy();
    expect(button("Join")).toBeTruthy();
    noButton(/^Add/);
    noButton(/^Remove/);
    noButton("Keep it");
    cleanup();
    // A call with no name and no link: still said, nothing to press.
    render(<Row call={{ kind: "other", name: "", join: null, state: "ready" }} />);
    expect(within(group()).getAllByText("Video call").length).toBe(2);
    expect(within(group()).queryAllByRole("button")).toEqual([]);
  });

  it("Join opens the link", () => {
    openUrl.mockResolvedValue(undefined);
    render(<Row call={meet()} value />);
    fireEvent.click(button("Join with Google Meet"));
    expect(openUrl).toHaveBeenCalledWith(LINK);
    cleanup();
    render(<Row call={zoom} />);
    fireEvent.click(button("Join"));
    expect(openUrl).toHaveBeenLastCalledWith("https://zoom.us/j/987");
  });

  // Review round 1 of PR 1: a browser that didn't open is said under the row, pointing to Copy link
  // beside Join, and pressing Join again clears it until the answer comes.
  it("Join that didn't open says so", async () => {
    openUrl.mockRejectedValueOnce("Couldn't open the link (NotFound).");
    render(<Row call={meet()} value />);
    expect(within(group()).queryByText(JOIN_FAILED)).toBeNull();
    fireEvent.click(button("Join with Google Meet"));
    expect(await within(group()).findByText(JOIN_FAILED)).toBeTruthy();
    expect(within(group()).getByText(JOIN_FAILED).getAttribute("role")).toBe("status");
    openUrl.mockResolvedValueOnce(undefined);
    fireEvent.click(button("Join with Google Meet"));
    expect(within(group()).queryByText(JOIN_FAILED)).toBeNull();
  });

  // Critique C2-M1: a guest, or anyone on another service's call, can still join.
  it("disabled turns off Add, Remove, Don't add and Keep it, never Join", () => {
    render(<Row disabled />);
    expect(button("Add Google Meet video conferencing")).toHaveProperty("disabled", true);
    cleanup();
    render(<Row call={meet()} value disabled />);
    expect(button("Remove the video call")).toHaveProperty("disabled", true);
    expect(button("Join with Google Meet")).toHaveProperty("disabled", false);
    cleanup();
    render(<Row call={meet()} value={false} disabled />);
    expect(button("Keep it")).toHaveProperty("disabled", true);
    cleanup();
    render(<Row value disabled />);
    expect(button("Don't add the video call")).toHaveProperty("disabled", true);
    cleanup();
    render(<Row call={zoom} disabled />);
    expect(button("Join")).toHaveProperty("disabled", false);
  });

  // Critique C2-M2: focus follows a press to the button that took its place, and the line saying
  // what Save will do is read out.
  it("moves focus to the button that replaced the one pressed, and reads out the change", () => {
    render(<Row />);
    fireEvent.click(button("Add Google Meet video conferencing"));
    expect(document.activeElement).toBe(button("Don't add the video call"));
    fireEvent.click(button("Don't add the video call"));
    expect(document.activeElement).toBe(button("Add Google Meet video conferencing"));
    cleanup();
    render(<Row call={meet()} value />);
    fireEvent.click(button("Remove the video call"));
    expect(document.activeElement).toBe(button("Keep it"));
    const line = within(group()).getByText("The video call will be removed when you save.");
    expect(line.getAttribute("aria-live")).toBe("polite");
    fireEvent.click(button("Keep it"));
    expect(document.activeElement).toBe(button("Remove the video call"));
  });

  it("never takes focus when the value changes without a press", () => {
    const { rerender } = render(
      <MeetField call={null} value={false} offered onChange={() => {}} />,
    );
    rerender(<MeetField call={null} value offered onChange={() => {}} />);
    expect(document.activeElement).toBe(document.body);
  });

  it("the call's name renders as text", () => {
    render(<Row call={{ ...zoom, name: "<b>x</b>" }} />);
    expect(within(group()).getByText("<b>x</b>")).toBeTruthy();
    expect(group().querySelector("b")).toBeNull();
  });

  // C18b: a new event's note that its calendar turned the call off shows even with nothing to press.
  it("a note shows where the calendar offers no Meet", () => {
    render(
      <MeetField
        call={null}
        value={false}
        offered={false}
        onChange={() => {}}
        note="Work doesn't offer Google Meet, so the video call is off."
      />,
    );
    expect(
      within(group()).getByText("Work doesn't offer Google Meet, so the video call is off."),
    ).toBeTruthy();
    expect(within(group()).queryByRole("button")).toBeNull();
  });
});

// --- quick create's size (C18b) ---

describe("MeetField compact", () => {
  /** Quick create's button, its value following `onChange`. */
  function Compact({
    offered = true,
    disabled,
    onPointerPress,
  }: {
    offered?: boolean;
    disabled?: boolean;
    onPointerPress?: () => void;
  }) {
    const [on, setOn] = useState(false);
    return (
      <MeetField
        compact
        call={null}
        value={on}
        offered={offered}
        disabled={disabled}
        onChange={setOn}
        onPointerPress={onPointerPress}
        note="never shown here"
      />
    );
  }
  const meetButton = () => screen.getByRole("button", { name: "Google Meet" });

  it("compact is one pressed-or-not button, hidden where Meet isn't offered", () => {
    render(<Compact />);
    expect(meetButton().getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(meetButton(), { detail: 1 });
    expect(meetButton().getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(meetButton(), { detail: 1 });
    expect(meetButton().getAttribute("aria-pressed")).toBe("false");
    expect(screen.queryByRole("group")).toBeNull();
    expect(screen.queryByText("never shown here")).toBeNull();
    cleanup();
    const { container } = render(<Compact offered={false} />);
    expect(container.textContent).toBe("");
    cleanup();
    render(<Compact disabled />);
    expect(meetButton()).toHaveProperty("disabled", true);
  });

  // Critique C2-M3: a click hands focus back (quick create's title, where Enter saves); a key press
  // on the button leaves it there.
  it("says a pointer press, never a key press", () => {
    const onPointerPress = vi.fn();
    render(<Compact onPointerPress={onPointerPress} />);
    fireEvent.click(meetButton(), { detail: 0 });
    expect(onPointerPress).not.toHaveBeenCalled();
    expect(meetButton().getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(meetButton(), { detail: 1 });
    expect(onPointerPress).toHaveBeenCalledTimes(1);
  });
});
