// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Voice input's row in Settings → Storage (Mods spec D8): the switch and the speech engine in one
// place. Turn on installs the engine (the backend writes the switch only once that worked), Turn off
// asks first and offers to remove the engine and the speech model, ticked, and Remove is offered
// only while voice is off.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { StorageComponent, StorageReport } from "../lib/types";

const listStorageComponents = vi.fn();
const setVoiceSwitch = vi.fn();
const getVoiceSwitch = vi.fn();
const removeStorageComponent = vi.fn();
const installOptionalOcr = vi.fn();
const unlisten = () => Promise.resolve(() => {});
vi.mock("../lib/ipc", () => ({
  listStorageComponents: () => listStorageComponents(),
  setVoiceSwitch: (on: boolean, remove: boolean) => setVoiceSwitch(on, remove),
  getVoiceSwitch: () => getVoiceSwitch(),
  removeStorageComponent: (id: string) => removeStorageComponent(id),
  installOptionalOcr: () => installOptionalOcr(),
  onOcrInstall: () => unlisten(),
  onVoiceInstall: () => unlisten(),
}));

// The primitives reach for `useTheme`; the real provider pulls in IPC.
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { switchActions, voiceRemovable } from "../lib/storageSwitch";
import { StorageSettings } from "./StorageSettings";

const row = (over: Partial<StorageComponent>): StorageComponent => ({
  id: "voice",
  label: "Voice input (speech engine)",
  detail: "Dictate into the chat box with the mic.",
  size_bytes: 100,
  approximate: false,
  child: false,
  status: "removable",
  blockers: [],
  manage: null,
  note: null,
  switch: true,
  ...over,
});

const report = (...components: StorageComponent[]): StorageReport => ({
  total_bytes: components.reduce((n, c) => n + c.size_bytes, 0),
  components,
});

beforeEach(() => {
  for (const f of [
    listStorageComponents,
    setVoiceSwitch,
    getVoiceSwitch,
    removeStorageComponent,
    installOptionalOcr,
  ])
    f.mockReset();
});
afterEach(cleanup);

describe("switchActions", () => {
  it("offers Turn off while on, beside a repair when the engine is missing or stale", () => {
    expect(switchActions(row({ switch: true, status: "removable" }))).toEqual(["turn-off"]);
    expect(switchActions(row({ switch: true, status: "stale" }))).toEqual([
      "install-again",
      "turn-off",
    ]);
    expect(switchActions(row({ switch: true, status: "installable" }))).toEqual([
      "install",
      "turn-off",
    ]);
  });

  it("offers Turn on while off, and Remove only for an engine still on disk", () => {
    expect(switchActions(row({ switch: false, status: "installable" }))).toEqual(["turn-on"]);
    expect(switchActions(row({ switch: false, status: "removable" }))).toEqual([
      "turn-on",
      "remove",
    ]);
    expect(switchActions(row({ switch: false, status: "stale" }))).toEqual(["turn-on", "remove"]);
  });

  it("is nothing for a row without a switch", () => {
    expect(switchActions(row({ id: "ocr", switch: null }))).toEqual([]);
  });
});

describe("voiceRemovable", () => {
  it("counts the engine on disk and the speech model, never an engine's install estimate", () => {
    const model = row({ id: "whisper", switch: null, size_bytes: 40 });
    expect(voiceRemovable(report(row({ size_bytes: 100 }), model))).toEqual({
      any: true,
      bytes: 140,
    });
    expect(
      voiceRemovable(report(row({ status: "installable", size_bytes: 999, approximate: true }))),
    ).toEqual({ any: false, bytes: 0 });
    expect(voiceRemovable(report(row({ status: "installable", size_bytes: 999 }), model))).toEqual({
      any: true,
      bytes: 40,
    });
  });

  // Someone who never dictated: the backend sends no Speech model row (it measures the Whisper
  // folder alone, never the search and OCR models beside it), so switching off frees nothing, and
  // the search model's own row is never counted as voice's.
  it("offers nothing to free without the engine or the speech model", () => {
    const search = row({ id: "embedder", switch: null, status: "in_use", size_bytes: 500 });
    expect(
      voiceRemovable(
        report(row({ status: "installable", size_bytes: 999, approximate: true }), search),
      ),
    ).toEqual({ any: false, bytes: 0 });
    expect(voiceRemovable(report(row({ size_bytes: 100 }), search))).toEqual({
      any: true,
      bytes: 100,
    });
  });
});

describe("the voice row", () => {
  it("turns voice off and, as ticked, removes the engine and the model", async () => {
    listStorageComponents.mockResolvedValue(
      report(row({}), row({ id: "whisper", label: "Speech model (Whisper)", switch: null })),
    );
    setVoiceSwitch.mockResolvedValue({ on: false });
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Turn off" }));
    const dialog = screen.getByRole("dialog");
    const remove = within(dialog).getByRole("checkbox") as HTMLInputElement;
    expect(remove.checked).toBe(true);
    fireEvent.click(within(dialog).getByRole("button", { name: "Turn off" }));

    await waitFor(() => expect(setVoiceSwitch).toHaveBeenCalledWith(false, true));
    // The row is re-read afterwards, so it shows what is left.
    await waitFor(() => expect(listStorageComponents).toHaveBeenCalledTimes(2));
  });

  it("can turn voice off and keep the engine", async () => {
    listStorageComponents.mockResolvedValue(report(row({})));
    setVoiceSwitch.mockResolvedValue({ on: false });
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Turn off" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("checkbox"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(setVoiceSwitch).toHaveBeenCalledWith(false, false));
  });

  it("offers no removal when there is nothing on disk to remove", async () => {
    listStorageComponents.mockResolvedValue(
      report(row({ status: "installable", approximate: true })),
    );
    setVoiceSwitch.mockResolvedValue({ on: false });
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Turn off" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).queryByRole("checkbox")).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(setVoiceSwitch).toHaveBeenCalledWith(false, false));
  });

  it("turns voice on, which installs the engine, then re-reads the row", async () => {
    listStorageComponents.mockResolvedValue(
      report(row({ switch: false, status: "installable", approximate: true })),
    );
    setVoiceSwitch.mockResolvedValue({ on: true });
    render(<StorageSettings onNavigate={() => {}} />);

    expect(screen.queryByRole("button", { name: "Remove" })).toBeNull();
    fireEvent.click(await screen.findByRole("button", { name: "Turn on" }));
    await waitFor(() => expect(setVoiceSwitch).toHaveBeenCalledWith(true, false));
    await waitFor(() => expect(listStorageComponents).toHaveBeenCalledTimes(2));
  });

  it("shows a failed install and leaves the row as it was", async () => {
    listStorageComponents.mockResolvedValue(
      report(row({ switch: false, status: "installable", approximate: true })),
    );
    setVoiceSwitch.mockRejectedValue("could not download the speech engine");
    getVoiceSwitch.mockResolvedValue({ on: false });
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Turn on" }));
    expect(await screen.findByText(/could not download the speech engine/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Turn on" })).toBeTruthy();
  });

  it("removes an engine left behind while voice is off, through the Storage removal", async () => {
    listStorageComponents.mockResolvedValue(report(row({ switch: false, status: "removable" })));
    removeStorageComponent.mockResolvedValue(undefined);
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/Remove the speech engine\?/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(removeStorageComponent).toHaveBeenCalledWith("voice"));
    expect(setVoiceSwitch).not.toHaveBeenCalled();
  });
});
