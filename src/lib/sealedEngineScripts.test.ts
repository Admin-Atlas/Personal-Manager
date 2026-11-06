// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The webview engine's two init scripts (src-tauri/src/mods/*.js, Mods spec §1.4) run before the
// page's own scripts in every document of PM's windows: `top_frame.js` in the top frame, and
// `webrtc_off.js` in every frame. A script that doesn't parse fails silently in the webview, and one
// that keys off the wrong origin either stops PM's own page or leaves a mod its WebRTC. So both run
// here, for real, against stand-in globals. The Rust side (mods/engine.rs) pins how it wraps
// `top_frame.js`; the wrapper below is the same shape.

import { describe, expect, it, vi } from "vitest";

const SOURCES = import.meta.glob<string>("../../src-tauri/src/mods/*.js", {
  query: "?raw",
  import: "default",
  eager: true,
});

function source(name: string): string {
  const text = SOURCES[`../../src-tauri/src/mods/${name}`];
  if (text === undefined) throw new Error(`no ${name}`);
  return text;
}

interface Place {
  /** `self.origin`: "null" in a sandboxed frame. */
  origin: string;
  protocol: string;
  /** `location.host`: the hostname plus a non-default port. */
  host: string;
  top?: boolean;
}

function locationOf(p: Place) {
  return { protocol: p.protocol, host: p.host, hostname: p.host.split(":")[0] };
}

describe("top_frame.js", () => {
  /** Run it as `top_frame_script` does, and say whether it stopped the document. */
  function stops(p: Place, cfg: { origins: string[]; dev: boolean }): boolean {
    const stop = vi.fn();
    const global: Record<string, unknown> = { origin: p.origin, stop };
    const top = p.top === false ? {} : global;
    const run = new Function(
      "PM_TOP_FRAME",
      "self",
      "top",
      "window",
      "location",
      source("top_frame.js"),
    );
    run(cfg, global, top, global, locationOf(p));
    return stop.mock.calls.length > 0;
  }

  const webkit = { origins: ["tauri://localhost"], dev: false };
  const webview2 = { origins: ["http://tauri.localhost"], dev: false };

  it("leaves PM's own page alone on every engine", () => {
    expect(
      stops({ origin: "tauri://localhost", protocol: "tauri:", host: "localhost" }, webkit),
    ).toBe(false);
    expect(
      stops(
        { origin: "http://tauri.localhost", protocol: "http:", host: "tauri.localhost" },
        webview2,
      ),
    ).toBe(false);
  });

  it("stops a mod's page at the top, sandboxed or not", () => {
    expect(stops({ origin: "null", protocol: "pmmod:", host: "localhost" }, webkit)).toBe(true);
    expect(
      stops({ origin: "pmmod://localhost", protocol: "pmmod:", host: "localhost" }, webkit),
    ).toBe(true);
    expect(
      stops(
        { origin: "http://pmmod.localhost", protocol: "http:", host: "pmmod.localhost" },
        webview2,
      ),
    ).toBe(true);
  });

  it("stops a sandboxed copy of a PM URL, a web page and about:blank", () => {
    expect(stops({ origin: "null", protocol: "tauri:", host: "localhost" }, webkit)).toBe(true);
    expect(
      stops({ origin: "https://example.com", protocol: "https:", host: "example.com" }, webkit),
    ).toBe(true);
    expect(stops({ origin: "tauri://localhost", protocol: "about:", host: "" }, webkit)).toBe(true);
    // WebView2's name for PM's scheme is just a web URL on WebKit.
    expect(
      stops(
        { origin: "http://tauri.localhost", protocol: "http:", host: "tauri.localhost" },
        webkit,
      ),
    ).toBe(true);
  });

  it("lets Vite's page through only in a dev build", () => {
    const vite = { origin: "http://localhost:1420", protocol: "http:", host: "localhost:1420" };
    expect(stops(vite, { ...webkit, dev: true })).toBe(false);
    expect(
      stops(
        { ...vite, origin: "http://127.0.0.1:1420", host: "127.0.0.1:1420" },
        { ...webkit, dev: true },
      ),
    ).toBe(false);
    expect(stops(vite, webkit)).toBe(true);
    expect(
      stops(
        {
          origin: "http://localhost.example.com",
          protocol: "http:",
          host: "localhost.example.com",
        },
        { ...webkit, dev: true },
      ),
    ).toBe(true);
  });

  it("does nothing in a subframe, where WebView2 injects it too", () => {
    expect(
      stops({ origin: "null", protocol: "pmmod:", host: "localhost", top: false }, webkit),
    ).toBe(false);
  });
});

describe("webrtc_off.js", () => {
  // The constructors the spike removed, every one.
  const NAMES = [
    "RTCPeerConnection",
    "webkitRTCPeerConnection",
    "RTCDataChannel",
    "RTCSessionDescription",
    "RTCIceCandidate",
    "RTCRtpSender",
    "RTCRtpReceiver",
    "RTCRtpTransceiver",
    "RTCDtlsTransport",
    "RTCIceTransport",
    "RTCSctpTransport",
    "RTCCertificate",
    "RTCEncodedAudioFrame",
    "RTCEncodedVideoFrame",
  ];

  function runIn(p: Place) {
    const getUserMedia = () => undefined;
    const global: Record<string, unknown> = {
      origin: p.origin,
      navigator: { mediaDevices: { getUserMedia } },
    };
    for (const name of NAMES) global[name] = function () {};
    const run = new Function("self", "location", source("webrtc_off.js"));
    run(global, locationOf(p));
    return { global, getUserMedia };
  }

  function expectRemoved(global: Record<string, unknown>) {
    for (const name of NAMES) {
      expect(global[name], name).toBeUndefined();
      const pinned = Object.getOwnPropertyDescriptor(global, name);
      expect(pinned?.configurable, name).toBe(false);
      expect(pinned?.writable, name).toBe(false);
    }
    expect(global.__pmPreamble).toBe(true);
  }

  it("removes WebRTC from a sandboxed frame and from a mod URL on either engine", () => {
    expectRemoved(runIn({ origin: "null", protocol: "pmmod:", host: "localhost" }).global);
    expectRemoved(
      runIn({ origin: "pmmod://localhost", protocol: "pmmod:", host: "localhost" }).global,
    );
    expectRemoved(
      runIn({ origin: "http://pmmod.localhost", protocol: "http:", host: "pmmod.localhost" })
        .global,
    );
    // Opaque is enough: a frame that lost its URL's identity is still a mod's.
    expectRemoved(runIn({ origin: "null", protocol: "about:", host: "" }).global);
  });

  it("leaves a mod's getUserMedia where it was", () => {
    const { global, getUserMedia } = runIn({
      origin: "null",
      protocol: "pmmod:",
      host: "localhost",
    });
    expect(
      (global.navigator as { mediaDevices: { getUserMedia: unknown } }).mediaDevices.getUserMedia,
    ).toBe(getUserMedia);
  });

  it("leaves PM's own page its WebRTC", () => {
    for (const p of [
      { origin: "tauri://localhost", protocol: "tauri:", host: "localhost" },
      { origin: "http://tauri.localhost", protocol: "http:", host: "tauri.localhost" },
      { origin: "http://localhost:1420", protocol: "http:", host: "localhost:1420" },
    ]) {
      const { global } = runIn(p);
      for (const name of NAMES) expect(typeof global[name], `${p.origin} ${name}`).toBe("function");
      expect(global.__pmPreamble).toBeUndefined();
    }
  });
});
