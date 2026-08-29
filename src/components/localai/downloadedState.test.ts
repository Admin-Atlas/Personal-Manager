// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "Already downloaded" panel's ladder. Tested here rather than through a render because the
// branch that was WRONG — a packaged Linux Ollama whose store PM is not allowed to read — cannot be
// produced from a component test without a service account and a running server. That is exactly
// why it shipped broken: on Bobby's Fedora laptop, with Ollama serving two models out of
// /usr/share/ollama/.ollama/models at mode 0700, the panel said "No model folder found for Ollama,
// LM Studio and Hugging Face".

import { describe, expect, it } from "vitest";
import { downloadedState, type DownloadedInputs } from "./downloadedState";

/** Nothing anywhere: no endpoint, no folder, no denial. The floor every case is built from. */
const NOTHING: DownloadedInputs = {
  unservedCount: 0,
  endpointInventory: null,
  foundRunners: [],
  diskFound: 0,
  blocked: [],
};

const OLLAMA_BLOCKED = {
  source: "ollama" as const,
  path: "/usr/share/ollama/.ollama/models",
};

describe("downloadedState", () => {
  it("lists whenever there is something unserved to list", () => {
    expect(downloadedState({ ...NOTHING, unservedCount: 1 }).kind).toBe("list");
  });

  it("says the server holds them rather than reporting nothing downloaded", () => {
    // The regression that started this: with an Ollama endpoint, `/v1/models` lists what has been
    // PULLED, so everything downloaded is also served and `unservedCount` is structurally 0 forever.
    // Reading that as "you have nothing" is how a machine with two models was told it had none.
    const s = downloadedState({ ...NOTHING, endpointInventory: 2 });
    expect(s).toEqual({ kind: "endpointHasAll", count: 2 });
  });

  it("names an unreadable store instead of claiming no folder exists", () => {
    const s = downloadedState({ ...NOTHING, blocked: [OLLAMA_BLOCKED] });
    expect(s).toEqual({ kind: "blocked", root: OLLAMA_BLOCKED });
  });

  it("only says no folder was found when neither rung found anything at all", () => {
    expect(downloadedState(NOTHING).kind).toBe("noFolder");
  });

  it("separates a server with nothing pulled from a server that never answered", () => {
    // `null` and `0` must not collapse: the first is a wrong address, the second is a first-time
    // installer who has done everything right so far.
    expect(downloadedState({ ...NOTHING, endpointInventory: 0 }).kind).toBe("endpointEmpty");
    expect(downloadedState({ ...NOTHING, endpointInventory: null }).kind).toBe("noFolder");
  });

  it("separates an empty runner folder from one whose models are all served", () => {
    const found = { ...NOTHING, foundRunners: ["LM Studio"] };
    expect(downloadedState({ ...found, diskFound: 0 }).kind).toBe("folderEmpty");
    expect(downloadedState({ ...found, diskFound: 3 }).kind).toBe("allServed");
  });

  describe("the order of the ladder", () => {
    it("prefers the endpoint's answer over any folder probe", () => {
      // The endpoint is the stronger rung: the server is the one process guaranteed to be allowed
      // to read its own store, so a folder probe that disagrees is describing PM's permissions.
      const s = downloadedState({
        ...NOTHING,
        endpointInventory: 2,
        foundRunners: ["LM Studio"],
        diskFound: 5,
        blocked: [OLLAMA_BLOCKED],
      });
      expect(s.kind).toBe("endpointHasAll");
    });

    it("still describes a folder that has models when the server is empty", () => {
      // Deliberately the other way round from the case above. "Your server has nothing" is only
      // worth saying when nothing else has anything — someone with three LM Studio models and a
      // freshly installed Ollama should hear about the three.
      const s = downloadedState({
        ...NOTHING,
        endpointInventory: 0,
        foundRunners: ["LM Studio"],
        diskFound: 3,
      });
      expect(s.kind).toBe("allServed");
    });

    it("explains the denial only when there is an absence left to explain", () => {
      // `blocked` sits low because it accounts for something missing. With a readable folder in
      // hand there is nothing missing to account for.
      const s = downloadedState({
        ...NOTHING,
        foundRunners: ["LM Studio"],
        diskFound: 2,
        blocked: [OLLAMA_BLOCKED],
      });
      expect(s.kind).toBe("allServed");
    });

    it("never reaches noFolder while a blocked root is known", () => {
      // The single assertion that would have caught the shipped bug.
      expect(downloadedState({ ...NOTHING, blocked: [OLLAMA_BLOCKED] }).kind).not.toBe("noFolder");
    });
  });
});
