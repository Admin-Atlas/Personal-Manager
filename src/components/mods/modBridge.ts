// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The bridge between a mod's sealed frame and PM (Mods spec §1.4, "The bridge"): what PM accepts
// from a frame, what it sends back, and the bounds it holds a frame to. Pure; ModFrame.tsx wires it
// to the frame and to `modCall` in ipc.ts.
//
// The protocol, version 1:
// - The frame asks for its port on its parent: `parent.postMessage({ pm: "connect", version: 1 }, "*")`.
//   PM answers only the frame it mounted (the message's `source` is that iframe's window), only while
//   the frame's origin is opaque, and once per session: it posts `{ pm: "port", version: 1 }` to the
//   frame with a MessagePort transferred. Both sides post with "*", because an opaque origin can't be
//   named. Every mod's origin is "null", so the origin can't tell two mods apart; the `source` does.
// - Over the port the frame sends a call, `{ id, method, args }`, or a height request, `{ resize }`.
//   PM answers a call with `{ id, ok: true, result }` or `{ id, ok: false, error }`, and never answers
//   a height request.
//
// Nothing a frame sends names a mod or a session. PM calls under the session it opened for that frame,
// and Rust resolves the session to its mod (`mod_call`). A message PM can't read is dropped, or, when it
// carries a usable id, refused with a reason.

export const BRIDGE_VERSION = 1;

/** Calls one frame may have waiting at once. More is a mod flooding PM's page, which runs every call. */
export const MAX_IN_FLIGHT = 32;

/** A call's arguments, as JSON text. Tauri reads the whole payload before Rust can refuse it, so the
 *  page holds the line. */
export const MAX_ARGS_CHARS = 1_048_576;

/** The heights a frame may ask for, in CSS pixels. */
export const MIN_FRAME_HEIGHT = 64;
export const MAX_FRAME_HEIGHT = 4096;

/** Method names: what the host API's dotted names look like, bounded. Rust checks each against its
 *  dispatch table; this only keeps junk off the IPC channel. */
const METHOD = /^[A-Za-z][A-Za-z0-9._-]{0,127}$/;

/** A reason sent back to a frame is clipped: PM's errors are short, and a mod has no use for more. */
const MAX_ERROR_CHARS = 500;

/** PM's answer to a connect request. Frozen: it is posted to every frame as is. */
export const PORT_MESSAGE = Object.freeze({ pm: "port", version: BRIDGE_VERSION });

/** What PM does with one message from a frame's port. */
export type PortMessage =
  | { kind: "call"; id: number; method: string; args: unknown }
  | { kind: "resize"; height: number }
  | { kind: "refuse"; id: number; error: string }
  | { kind: "drop" };

const DROP: PortMessage = { kind: "drop" };

/** Messages arrive structured-cloned: plain data, with no getters or prototypes of the sender's. */
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Whether a message on PM's window is a frame asking for its port. The caller has already checked
 *  that it came from the frame PM mounted. */
export function isConnectRequest(data: unknown): boolean {
  return isRecord(data) && data.pm === "connect" && data.version === BRIDGE_VERSION;
}

/** A requested height held to PM's bounds; `null` for anything that isn't a finite number. */
export function clampFrameHeight(requested: unknown): number | null {
  if (typeof requested !== "number" || !Number.isFinite(requested)) return null;
  return Math.min(MAX_FRAME_HEIGHT, Math.max(MIN_FRAME_HEIGHT, Math.round(requested)));
}

/** A call's arguments as exactly the JSON PM will send: measured, then parsed back, so what reaches
 *  Rust is what was measured. Structured clone carries more than JSON (a BigInt, a cycle), and none of
 *  it is an argument PM's API takes. */
export function encodeArgs(
  args: unknown,
): { ok: true; value: unknown } | { ok: false; error: string } {
  let json: string;
  try {
    json = JSON.stringify(args === undefined ? null : args);
  } catch {
    return { ok: false, error: "A call's arguments must be plain JSON." };
  }
  if (json.length > MAX_ARGS_CHARS) {
    return { ok: false, error: "A call's arguments are too large." };
  }
  return { ok: true, value: JSON.parse(json) as unknown };
}

/** What PM makes of one message from a frame's port. */
export function readPortMessage(data: unknown): PortMessage {
  if (!isRecord(data)) return DROP;
  if ("resize" in data) {
    const height = clampFrameHeight(data.resize);
    return height === null ? DROP : { kind: "resize", height };
  }
  const id = data.id;
  if (typeof id !== "number" || !Number.isSafeInteger(id) || id < 0) return DROP;
  const method = data.method;
  if (typeof method !== "string" || !METHOD.test(method)) {
    return { kind: "refuse", id, error: "PM couldn't read that call's method." };
  }
  const args = encodeArgs(data.args);
  if (!args.ok) return { kind: "refuse", id, error: args.error };
  return { kind: "call", id, method, args: args.value };
}

/** A refusal's text for the frame: the backend's message, clipped. */
export function errorText(e: unknown): string {
  const text = typeof e === "string" ? e : e instanceof Error ? e.message : "PM refused that call.";
  return text.length > MAX_ERROR_CHARS ? `${text.slice(0, MAX_ERROR_CHARS)}…` : text;
}

/** The answer to a call that succeeded. */
export function callResult(id: number, result: unknown) {
  return { id, ok: true as const, result };
}

/** The answer to a call PM or the backend refused. */
export function callError(id: number, error: string) {
  return { id, ok: false as const, error: errorText(error) };
}
