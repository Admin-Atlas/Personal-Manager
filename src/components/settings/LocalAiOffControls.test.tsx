// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// AI & Models while Local AI is off (D7): Bobby's "tell them to pick a model" for each role that
// was routed to a local model, the way back on, and "Forget server and token" still reachable.
// Plus the switch at the top of the Local AI tab, whose confirm says models in Ollama stay there.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LocalAiSwitch } from "../../lib/types";

const getLocalAiSwitch = vi.fn();
const setLocalAiSwitch = vi.fn();
const setLocalLlmRouting = vi.fn();
const clearLocalLlmEndpoint = vi.fn();
vi.mock("../../lib/ipc", () => ({
  getLocalAiSwitch: () => getLocalAiSwitch(),
  setLocalAiSwitch: (on: boolean) => setLocalAiSwitch(on),
  setLocalLlmRouting: (role: string, pref: string) => setLocalLlmRouting(role, pref),
  clearLocalLlmEndpoint: () => clearLocalLlmEndpoint(),
}));

// The primitives reach for `useTheme`; the real provider pulls in IPC.
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { LOCAL_AI_OFF_MESSAGE, LocalAiOffRow, LocalAiPickPrompt } from "./LocalAiOffControls";
import { LocalAiSwitchBar } from "../localai/LocalAiSwitchBar";

const sw = (over: Partial<LocalAiSwitch> = {}): LocalAiSwitch => ({
  on: false,
  chat_needs_model: false,
  background_needs_model: false,
  server_saved: false,
  ...over,
});

beforeEach(() => {
  for (const f of [getLocalAiSwitch, setLocalAiSwitch, setLocalLlmRouting, clearLocalLlmEndpoint])
    f.mockReset();
});
afterEach(cleanup);

describe("Bobby's words", () => {
  // The backend refuses with `local_ai_switch::LOCAL_AI_OFF_MESSAGE` and AI & Models shows this
  // copy of it, so the two can't drift apart.
  it("are the backend's own, word for word", () => {
    const source = Object.values(
      import.meta.glob<string>("../../../src-tauri/src/local_ai_switch.rs", {
        query: "?raw",
        import: "default",
        eager: true,
      }),
    );
    expect(source).toHaveLength(1);
    const backend = source[0].match(/const LOCAL_AI_OFF_MESSAGE: &str =\s*"([^"]+)";/)?.[1];
    expect(backend).toBe(LOCAL_AI_OFF_MESSAGE);
  });
});

describe("the pick-a-model prompt", () => {
  it("says Bobby's words and offers the cloud model for each role with nothing to answer it", async () => {
    setLocalLlmRouting.mockResolvedValue(undefined);
    getLocalAiSwitch.mockResolvedValue(sw({ background_needs_model: true }));
    const onError = vi.fn();
    render(
      <LocalAiPickPrompt
        sw={sw({ chat_needs_model: true, background_needs_model: true })}
        chatModel="inclusionai/ling-2.6-flash"
        backgroundModel={null}
        onError={onError}
      />,
    );
    expect(screen.getByText(LOCAL_AI_OFF_MESSAGE)).toBeTruthy();
    // The button names the model that will answer, or says "the cloud model" when it can't.
    const useChat = screen.getByRole("button", { name: "Use ling-2.6-flash" });
    expect(screen.getByRole("button", { name: "Use the cloud background model" })).toBeTruthy();

    fireEvent.click(useChat);
    await waitFor(() => expect(setLocalLlmRouting).toHaveBeenCalledWith("chat", "cloud"));
    // Then the switch is re-read, so the prompt can drop the role it just fixed.
    await waitFor(() => expect(getLocalAiSwitch).toHaveBeenCalled());
    expect(setLocalLlmRouting).not.toHaveBeenCalledWith("background", expect.anything());
    expect(onError).not.toHaveBeenCalledWith(expect.stringMatching(/./));
  });

  it("offers nothing for a role the cloud already answers, and nothing at all while on", () => {
    const { container, rerender } = render(
      <LocalAiPickPrompt
        sw={sw({ chat_needs_model: true })}
        chatModel="a/b"
        backgroundModel="c/d"
        onError={() => {}}
      />,
    );
    expect(screen.queryByRole("button", { name: "Use d" })).toBeNull();
    expect(screen.getByRole("button", { name: "Use b" })).toBeTruthy();
    rerender(
      <LocalAiPickPrompt sw={sw()} chatModel="a/b" backgroundModel="c/d" onError={() => {}} />,
    );
    expect(container.textContent).toBe("");
    rerender(
      <LocalAiPickPrompt
        sw={sw({ on: true, chat_needs_model: true })}
        chatModel="a/b"
        backgroundModel="c/d"
        onError={() => {}}
      />,
    );
    expect(container.textContent).toBe("");
  });

  it("says a failed pick where the tab shows errors", async () => {
    setLocalLlmRouting.mockRejectedValue("the vault is locked");
    const onError = vi.fn();
    render(
      <LocalAiPickPrompt
        sw={sw({ chat_needs_model: true })}
        chatModel={null}
        backgroundModel={null}
        onError={onError}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Use the cloud chat model" }));
    await waitFor(() => expect(onError).toHaveBeenCalledWith("the vault is locked"));
  });
});

describe("the Local AI: Off row", () => {
  it("turns Local AI back on and takes the reader to its tab", async () => {
    setLocalAiSwitch.mockResolvedValue(sw({ on: true }));
    const onTurnedOn = vi.fn();
    render(
      <LocalAiOffRow sw={sw({ server_saved: true })} onTurnedOn={onTurnedOn} onError={() => {}} />,
    );
    fireEvent.click(screen.getByRole("button", { name: /turn on/i }));
    await waitFor(() => expect(setLocalAiSwitch).toHaveBeenCalledWith(true));
    await waitFor(() => expect(onTurnedOn).toHaveBeenCalled());
  });

  it("keeps Forget server and token reachable while off, behind a confirm", async () => {
    clearLocalLlmEndpoint.mockResolvedValue(undefined);
    getLocalAiSwitch.mockResolvedValue(sw());
    render(<LocalAiOffRow sw={sw({ server_saved: true })} onError={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Forget server and token" }));
    expect(clearLocalLlmEndpoint).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Forget" }));
    await waitFor(() => expect(clearLocalLlmEndpoint).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(getLocalAiSwitch).toHaveBeenCalled());
  });

  it("has nothing to forget when no server or token is saved", () => {
    render(<LocalAiOffRow sw={sw()} onError={() => {}} />);
    expect(screen.queryByRole("button", { name: "Forget server and token" })).toBeNull();
    expect(screen.getByRole("button", { name: /turn on/i })).toBeTruthy();
  });

  it("says a switch that failed, and stays where it was", async () => {
    setLocalAiSwitch.mockRejectedValue("database lock poisoned");
    const onTurnedOn = vi.fn();
    const onError = vi.fn();
    render(<LocalAiOffRow sw={sw()} onTurnedOn={onTurnedOn} onError={onError} />);
    fireEvent.click(screen.getByRole("button", { name: /turn on/i }));
    await waitFor(() => expect(onError).toHaveBeenCalledWith("database lock poisoned"));
    expect(onTurnedOn).not.toHaveBeenCalled();
  });
});

describe("the switch at the top of the Local AI tab", () => {
  it("asks first, says models in Ollama stay there, and only then switches off", async () => {
    setLocalAiSwitch.mockResolvedValue(sw());
    render(<LocalAiSwitchBar />);
    const toggle = screen.getByRole("switch", { name: "Local AI" });
    expect(toggle.getAttribute("aria-checked")).toBe("true");

    fireEvent.click(toggle);
    expect(setLocalAiSwitch).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog");
    expect(dialog.textContent).toMatch(/Models you downloaded into Ollama stay there/);
    expect(dialog.textContent).toMatch(/Nothing moves to the cloud on its own/);
    expect(dialog.textContent).toMatch(/keeps your server's address and token/);
    // Back on is one click to the server, not to the models it cleared (D7).
    expect(dialog.textContent).toMatch(/You then pick your models again/);

    fireEvent.click(within(dialog).getByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(setLocalAiSwitch).toHaveBeenCalledWith(false));
  });

  it("leaves Local AI on when the confirm is cancelled", () => {
    render(<LocalAiSwitchBar />);
    fireEvent.click(screen.getByRole("switch", { name: "Local AI" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));
    expect(setLocalAiSwitch).not.toHaveBeenCalled();
  });
});
