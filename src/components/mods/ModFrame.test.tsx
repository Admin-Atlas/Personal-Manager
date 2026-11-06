// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// ModFrame's contract (Mods spec §1.4): the sealed iframe and PM's band, one session per mount, a port
// only for the frame PM mounted, calls relayed under PM's own session, a navigation ends the session,
// heights clamped, and the session closed on unmount. jsdom loads nothing into the frame, so the tests
// play the frame: they fire its `load` events, post its messages with the frame as `source`, and talk
// to PM over the port PM hands it.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ModFrame, type ModTier } from "./ModFrame";
import { MAX_FRAME_HEIGHT, MAX_IN_FLIGHT, MIN_FRAME_HEIGHT, PORT_MESSAGE } from "./modBridge";

const modFrameOpen = vi.fn();
const modFrameClose = vi.fn();
const modCall = vi.fn();

vi.mock("../../lib/ipc", () => ({
  modFrameOpen: (modId: string) => modFrameOpen(modId),
  modFrameClose: (session: string) => modFrameClose(session),
  modCall: (session: string, method: string, args: unknown) => modCall(session, method, args),
}));

const ENTRY = "pmmod://localhost/acme.tool/app/index.html?entry=" + "a".repeat(64);
const CONNECT = { pm: "connect", version: 1 };

let ports: MessagePort[] = [];

beforeEach(() => {
  vi.clearAllMocks();
  let n = 0;
  modFrameOpen.mockImplementation(() => {
    n += 1;
    return Promise.resolve({ entryUrl: ENTRY, session: `session-${n}` });
  });
  modFrameClose.mockResolvedValue(undefined);
  modCall.mockResolvedValue({ answer: 42 });
});

afterEach(() => {
  cleanup();
  for (const port of ports) port.close();
  ports = [];
});

function mount(tier: ModTier = "second") {
  return render(<ModFrame modId="acme.tool" name="Acme Tool" tier={tier} />);
}

const frameNow = () => document.querySelector("iframe");

async function theFrame(): Promise<HTMLIFrameElement> {
  return waitFor(() => {
    const frame = frameNow();
    if (!frame) throw new Error("no frame yet");
    return frame;
  });
}

/** Post `data` on PM's window as if it came from `source`. */
function postFrom(source: Window | null, data: unknown, origin = "null") {
  act(() => {
    window.dispatchEvent(new MessageEvent("message", { data, origin, source }));
  });
}

/** The frame's side of the handshake: ask for the port, and take the one PM transfers. */
function connect(frame: HTMLIFrameElement): { port: MessagePort; post: ReturnType<typeof vi.fn> } {
  const win = frame.contentWindow!;
  const post = vi.fn();
  vi.spyOn(win, "postMessage").mockImplementation(post);
  postFrom(win, CONNECT);
  expect(post).toHaveBeenCalledTimes(1);
  const [message, target, transfer] = post.mock.calls[0] as [unknown, string, MessagePort[]];
  expect(message).toEqual(PORT_MESSAGE);
  expect(target).toBe("*");
  expect(transfer).toHaveLength(1);
  ports.push(transfer[0]);
  return { port: transfer[0], post };
}

/** Send one message over the frame's port and wait for PM's answer. */
function ask(port: MessagePort, message: unknown): Promise<unknown> {
  return new Promise((resolve) => {
    port.onmessage = (e: MessageEvent) => resolve(e.data);
    port.postMessage(message);
  });
}

describe("ModFrame", () => {
  it("opens one session and mounts the sealed frame on its entry URL, under PM's band", async () => {
    mount();
    const frame = await theFrame();
    expect(modFrameOpen).toHaveBeenCalledTimes(1);
    expect(modFrameOpen).toHaveBeenCalledWith("acme.tool");
    expect(frame.getAttribute("src")).toBe(ENTRY);
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
    expect(frame.getAttribute("allow")).toBe("");
    expect(frame.hasAttribute("srcdoc")).toBe(false);
    const band = screen.getByText("Acme Tool", { selector: "span" }).parentElement!;
    expect(band.textContent).toBe("ModAcme ToolSecond party");
    // The band is PM's, outside the frame, and offers nothing to click.
    expect(frame.contains(band)).toBe(false);
    expect(band.querySelector("a, button, input")).toBeNull();
  });

  it("shows a tier it doesn't know as the least trusted one", async () => {
    render(<ModFrame modId="acme.tool" name="Acme Tool" tier={"atlas" as ModTier} />);
    await theFrame();
    expect(screen.getByText("Third party")).toBeTruthy();
  });

  it("draws the mod's name as text, never as markup", async () => {
    render(<ModFrame modId="acme.tool" name={'<img src=x onerror="alert(1)">'} tier="third" />);
    expect((await theFrame()).title).toBe('<img src=x onerror="alert(1)">');
    expect(document.querySelector("img")).toBeNull();
    expect(screen.getByText('<img src=x onerror="alert(1)">', { selector: "span" })).toBeTruthy();
  });

  it("hands its frame a port, once", async () => {
    mount();
    const frame = await theFrame();
    const { post } = connect(frame);
    // A second request in the same session gets nothing: the frame already holds its port.
    postFrom(frame.contentWindow, CONNECT);
    expect(post).toHaveBeenCalledTimes(1);
  });

  it("ignores a message from anything but its own frame", async () => {
    mount();
    const frame = await theFrame();
    const post = vi.spyOn(frame.contentWindow!, "postMessage").mockImplementation(() => {});
    const other = document.createElement("iframe");
    document.body.appendChild(other);
    postFrom(other.contentWindow, CONNECT);
    postFrom(window, CONNECT);
    postFrom(null, CONNECT);
    expect(post).not.toHaveBeenCalled();
    other.remove();
  });

  it("ignores its own frame when the frame's origin isn't opaque, or the request is malformed", async () => {
    mount();
    const frame = await theFrame();
    const post = vi.spyOn(frame.contentWindow!, "postMessage").mockImplementation(() => {});
    postFrom(frame.contentWindow, CONNECT, "pmmod://localhost");
    postFrom(frame.contentWindow, CONNECT, "http://pmmod.localhost");
    postFrom(frame.contentWindow, { pm: "connect", version: 2 });
    postFrom(frame.contentWindow, { pm: "connect" });
    postFrom(frame.contentWindow, "connect");
    expect(post).not.toHaveBeenCalled();
  });

  it("relays a call under PM's own session, whatever the message names", async () => {
    mount();
    const frame = await theFrame();
    const { port } = connect(frame);
    const answer = await ask(port, {
      id: 7,
      method: "storage.get",
      args: { key: "x" },
      session: "someone-elses",
      modId: "atlas.map",
    });
    expect(modCall).toHaveBeenCalledWith("session-1", "storage.get", { key: "x" });
    expect(answer).toEqual({ id: 7, ok: true, result: { answer: 42 } });
  });

  it("passes the backend's refusal back to the frame", async () => {
    modCall.mockRejectedValue("unknown method");
    mount();
    const { port } = connect(await theFrame());
    expect(await ask(port, { id: 1, method: "nope.never", args: null })).toEqual({
      id: 1,
      ok: false,
      error: "unknown method",
    });
  });

  it("refuses a call it can't read, without reaching the backend", async () => {
    mount();
    const { port } = connect(await theFrame());
    expect(await ask(port, { id: 2, method: "bad method!" })).toMatchObject({ id: 2, ok: false });
    expect(await ask(port, { id: 3, method: "x.y", args: 10n })).toMatchObject({
      id: 3,
      ok: false,
    });
    expect(modCall).not.toHaveBeenCalled();
  });

  it("refuses calls past the in-flight cap until earlier ones finish", async () => {
    modCall.mockImplementation(() => new Promise(() => {}));
    mount();
    const { port } = connect(await theFrame());
    for (let id = 0; id < MAX_IN_FLIGHT; id += 1) port.postMessage({ id, method: "a.b" });
    const answer = await ask(port, { id: 999, method: "a.b" });
    expect(answer).toEqual({ id: 999, ok: false, error: "Too many calls at once." });
    expect(modCall).toHaveBeenCalledTimes(MAX_IN_FLIGHT);
  });

  it("clamps a height request, and ignores one that isn't a number", async () => {
    mount();
    const frame = await theFrame();
    const { port } = connect(frame);
    const settle = () => new Promise((r) => setTimeout(r, 20));
    port.postMessage({ resize: 99_999 });
    await waitFor(() => expect(frame.style.height).toBe(`${MAX_FRAME_HEIGHT}px`));
    port.postMessage({ resize: -5 });
    await waitFor(() => expect(frame.style.height).toBe(`${MIN_FRAME_HEIGHT}px`));
    port.postMessage({ resize: 300.4 });
    await waitFor(() => expect(frame.style.height).toBe("300px"));
    port.postMessage({ resize: "900px; background: url(x)" });
    port.postMessage({ resize: Number.NaN });
    port.postMessage({ resize: Number.POSITIVE_INFINITY });
    await act(settle);
    expect(frame.style.height).toBe("300px");
    expect(frame.getAttribute("style")).toBe("height: 300px;");
  });

  it("treats a second load as a navigation: closes the session and stops the frame", async () => {
    mount();
    const frame = await theFrame();
    const { port } = connect(frame);
    fireEvent.load(frame);
    expect(modFrameClose).not.toHaveBeenCalled();
    fireEvent.load(frame);
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    expect(frameNow()).toBeNull();
    expect(screen.getByRole("status").textContent).toBe(
      "This mod reloaded itself and was stopped.",
    );
    // The port is dead: a call after the navigation reaches nothing.
    port.postMessage({ id: 1, method: "a.b" });
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(modCall).not.toHaveBeenCalled();
    // Unmounting later doesn't close it twice.
    cleanup();
    expect(modFrameClose).toHaveBeenCalledTimes(1);
  });

  it("gives a frame that navigated before it asked no port", async () => {
    mount();
    const frame = await theFrame();
    const post = vi.spyOn(frame.contentWindow!, "postMessage").mockImplementation(() => {});
    fireEvent.load(frame);
    fireEvent.load(frame);
    postFrom(frame.contentWindow, CONNECT);
    expect(post).not.toHaveBeenCalled();
  });

  it("closes the session when it unmounts", async () => {
    const { unmount } = mount();
    const { port } = connect(await theFrame());
    unmount();
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    port.postMessage({ id: 1, method: "a.b" });
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(modCall).not.toHaveBeenCalled();
  });

  it("closes a session that arrives after it unmounted", async () => {
    let resolve!: (v: { entryUrl: string; session: string }) => void;
    modFrameOpen.mockImplementation(() => new Promise((r) => (resolve = r)));
    const { unmount } = mount();
    unmount();
    await act(async () => resolve({ entryUrl: ENTRY, session: "late" }));
    expect(modFrameClose).toHaveBeenCalledWith("late");
  });

  it("under StrictMode, keeps exactly one session open", async () => {
    render(
      <StrictMode>
        <ModFrame modId="acme.tool" name="Acme Tool" tier="first" />
      </StrictMode>,
    );
    await theFrame();
    await waitFor(() => expect(modFrameClose).toHaveBeenCalledTimes(1));
    expect(modFrameOpen).toHaveBeenCalledTimes(2);
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    expect(document.querySelectorAll("iframe")).toHaveLength(1);
    expect(frameNow()!.getAttribute("src")).toBe(ENTRY);
  });

  it("says so when the mod can't be opened, and mounts no frame", async () => {
    modFrameOpen.mockRejectedValue("That mod isn't installed.");
    mount();
    expect((await screen.findByRole("alert")).textContent).toBe(
      "PM couldn't start this mod: That mod isn't installed.",
    );
    expect(document.querySelector("iframe")).toBeNull();
  });
});
