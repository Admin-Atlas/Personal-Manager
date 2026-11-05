// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mod's sealed frame under the band PM draws for it (Mods spec §1.4).
//
// - The band shows the mod's name as React text and its tier badge. It sits above the frame and
//   outside it, so nothing the mod draws can cover it, and a mod's page can't pass itself off as a PM
//   dialog. It carries no links and no buttons.
// - The frame is `<iframe src sandbox="allow-scripts" allow="">` and nothing more: the ESLint fence
//   allows an iframe only here and only in that shape (INVARIANTS.md I-24). Its `src` is the one-time
//   entry URL `mod_frame_open` mints, set once; a new session gets a new element (keyed by session).
// - A second `load` means the frame navigated. PM closes the session and drops the frame: main's
//   frame-src allows every mod's path, and a frame that could load another page could load another
//   mod's code under this one's grants.
// - The load count isn't the wall that keeps another document out of the frame, since a page that
//   navigates away before its own `load` hands "first load" to the next one, and the port to
//   whichever document connects first. What decides which documents can get in at all is in Rust
//   and the engine: main's frame-src names only the mod origins (WebKit enforces it on a frame's own
//   navigation, measured; the sealed-frame job measures data: and blob: on every engine), the
//   navigation guard admits a pmmod page only as a pending entry URL (WebKit), the scheme renders a
//   page only for its one-time token, and the sinkhole (Linux, Windows) or the content rule list
//   (macOS) takes every web URL off the network. So the next document can only be about:blank or
//   the engine's own error page, and neither runs anyone's script.
// - The bridge is modBridge.ts. The mod is the session's: PM calls under the session it opened for
//   this frame, never anything a message names.
// - A height request is a number PM clamps. Nothing a mod sends ever becomes an attribute, a class, a
//   style string or markup.
//
// Nothing renders this yet; the Mods tab (spec §8, PR-1 part 6) will.

import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { modCall, modFrameClose, modFrameOpen } from "../../lib/ipc";
import { Callout } from "../ui/Callout";
import { cn } from "../ui/cn";
import { toneSurface } from "../ui/tone";
import {
  MAX_IN_FLIGHT,
  PORT_MESSAGE,
  callError,
  callResult,
  errorText,
  isConnectRequest,
  readPortMessage,
} from "./modBridge";

/** A mod's trust tier (spec §1.1), as the backend names it. */
export type ModTier = "first" | "second" | "third";

const TIER_LABEL: Record<ModTier, string> = {
  first: "First party",
  second: "Second party",
  third: "Third party",
};

export interface ModFrameProps {
  /** The installed mod's id, from PM's own list. */
  modId: string;
  /** The mod's name, shown in the band as text. */
  name: string;
  tier: ModTier;
}

export function ModFrame(props: ModFrameProps) {
  // Another mod is another frame: remount, so no session, state or element carries over.
  return <SealedFrame key={props.modId} {...props} />;
}

type Phase =
  | { kind: "opening" }
  | { kind: "open"; session: string; entryUrl: string }
  | { kind: "stopped" }
  | { kind: "failed"; message: string };

/** One session's live state. Kept out of React state so every handler sees a change at once. */
interface Live {
  session: string;
  loads: number;
  port: MessagePort | null;
  inFlight: number;
  ended: boolean;
}

/** Closing is idempotent and best-effort: a session whose frame is gone has nobody left to call it,
 *  since only PM's page knows it. */
function closeQuietly(session: string) {
  modFrameClose(session).catch(() => {});
}

function endSession(live: Live) {
  if (live.ended) return;
  live.ended = true;
  live.port?.close();
  live.port = null;
  closeQuietly(live.session);
}

function reply(live: Live, message: unknown) {
  if (live.ended || !live.port) return;
  try {
    live.port.postMessage(message);
  } catch {
    // Every answer is plain JSON from the backend, so this can't fail to clone; if it ever did, the
    // frame's call simply goes unanswered.
  }
}

function SealedFrame({ modId, name, tier }: ModFrameProps) {
  const [phase, setPhase] = useState<Phase>({ kind: "opening" });
  const [height, setHeight] = useState<number | null>(null);
  const frameRef = useRef<HTMLIFrameElement>(null);
  const liveRef = useRef<Live | null>(null);

  // One session per mount. Under StrictMode's mount-unmount-mount the first open may resolve after its
  // cleanup ran; that session is closed the moment it arrives.
  useEffect(() => {
    let cancelled = false;
    modFrameOpen(modId).then(
      (opened) => {
        if (cancelled) {
          closeQuietly(opened.session);
          return;
        }
        liveRef.current = {
          session: opened.session,
          loads: 0,
          port: null,
          inFlight: 0,
          ended: false,
        };
        setPhase({ kind: "open", session: opened.session, entryUrl: opened.entryUrl });
      },
      (e: unknown) => {
        if (!cancelled) setPhase({ kind: "failed", message: errorText(e) });
      },
    );
    return () => {
      cancelled = true;
      const live = liveRef.current;
      liveRef.current = null;
      if (live) endSession(live);
    };
  }, [modId]);

  // A layout effect: it runs in the same task as the commit that mounts the frame, so the listener is
  // in place before the frame's first message (a task of its own) can arrive.
  const open = phase.kind === "open";
  useLayoutEffect(() => {
    if (!open) return;

    const onPortMessage = (live: Live, data: unknown) => {
      if (live.ended) return;
      const message = readPortMessage(data);
      switch (message.kind) {
        case "resize":
          setHeight(message.height);
          return;
        case "refuse":
          reply(live, callError(message.id, message.error));
          return;
        case "call": {
          if (live.inFlight >= MAX_IN_FLIGHT) {
            reply(live, callError(message.id, "Too many calls at once."));
            return;
          }
          live.inFlight += 1;
          void modCall(live.session, message.method, message.args)
            .then(
              (result) => reply(live, callResult(message.id, result)),
              (e: unknown) => reply(live, callError(message.id, errorText(e))),
            )
            .finally(() => {
              live.inFlight -= 1;
            });
          return;
        }
        case "drop":
          return;
      }
    };

    const onMessage = (event: MessageEvent) => {
      const frame = frameRef.current?.contentWindow;
      // Only the frame PM mounted, and only while its origin is opaque, as every sealed frame's is: a
      // frame whose origin isn't "null" has lost its sandbox, and PM doesn't talk to it.
      if (!frame || event.source !== frame || event.origin !== "null") return;
      if (!isConnectRequest(event.data)) return;
      const live = liveRef.current;
      if (!live || live.ended || live.port) return;
      const channel = new MessageChannel();
      live.port = channel.port1;
      channel.port1.onmessage = (e: MessageEvent) => onPortMessage(live, e.data);
      frame.postMessage(PORT_MESSAGE, "*", [channel.port2]);
    };

    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [open]);

  const onLoad = () => {
    const live = liveRef.current;
    if (!live || live.ended) return;
    live.loads += 1;
    if (live.loads > 1) {
      endSession(live);
      liveRef.current = null;
      setPhase({ kind: "stopped" });
    }
  };

  return (
    <section className="flex h-full min-h-0 flex-col" aria-label={`Mod: ${name}`}>
      <ModBand name={name} tier={tier} />
      {phase.kind === "open" && (
        <iframe
          key={phase.session}
          ref={frameRef}
          src={phase.entryUrl}
          sandbox="allow-scripts"
          allow=""
          title={name}
          className={cn("block w-full border-0", height === null ? "min-h-0 flex-1" : "flex-none")}
          style={height === null ? undefined : { height }}
          onLoad={onLoad}
        />
      )}
      {phase.kind === "opening" && <p className="px-4 py-3 text-sm text-ink3">Starting the mod…</p>}
      {phase.kind === "stopped" && (
        <Callout tone="warning" live className="m-4">
          This mod reloaded itself and was stopped.
        </Callout>
      )}
      {phase.kind === "failed" && (
        <Callout className="m-4">PM couldn't start this mod: {phase.message}</Callout>
      )}
    </section>
  );
}

/** The band above every mod frame: PM's own drawing, never the mod's. */
function ModBand({ name, tier }: { name: string; tier: ModTier }) {
  // A tier PM doesn't know is shown as the least trusted one.
  const known = Object.prototype.hasOwnProperty.call(TIER_LABEL, tier);
  const shown: ModTier = known ? tier : "third";
  return (
    <div className="flex min-w-0 flex-none items-center gap-2 border-b border-rule bg-panel px-3 py-1.5 text-xs">
      <span className="flex-none text-ink3">Mod</span>
      <span className="min-w-0 truncate font-medium text-ink" title={name}>
        {name}
      </span>
      <span
        className={cn(
          "flex-none rounded-full border px-2 py-px",
          shown === "third" ? undefined : "border-border2 text-ink2",
        )}
        style={shown === "third" ? toneSurface("warning") : undefined}
      >
        {TIER_LABEL[shown]}
      </span>
    </div>
  );
}
