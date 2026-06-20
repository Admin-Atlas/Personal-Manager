// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { transcribeAudio } from "./ipc";

/** idle → recording → transcribing → idle. */
export type RecorderState = "idle" | "recording" | "transcribing";

export interface Recorder {
  state: RecorderState;
  /** A user-friendly message for a mic/transcription failure, or null. */
  error: string | null;
  /** Begin capturing from the microphone. */
  start: () => Promise<void>;
  /** Stop capturing and transcribe; the text is delivered via `onTranscript`. */
  stop: () => void;
}

/**
 * Microphone capture for voice input (spec §4 P1). Records a clip in the webview
 * (`getUserMedia` + `MediaRecorder`), then hands the bytes to the sidecar's local
 * Whisper model via `transcribeAudio` — the audio never leaves the device. The
 * transcript is delivered through `onTranscript` so the caller can drop it into
 * the chat box for the user to review before sending; nothing is auto-sent.
 */
export function useRecorder(onTranscript: (text: string) => void): Recorder {
  const [state, setState] = useState<RecorderState>("idle");
  const [error, setError] = useState<string | null>(null);

  const recorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  const streamRef = useRef<MediaStream | null>(null);
  // True from the moment start() is invoked until the recorder is live, so a second
  // click during the getUserMedia await can't open (and orphan) a second mic stream.
  const startingRef = useRef(false);
  // Guards async state updates that may resolve after the component unmounts.
  const mountedRef = useRef(true);
  // Keep the latest callback so a long-running recording never calls a stale one.
  const onTranscriptRef = useRef(onTranscript);
  onTranscriptRef.current = onTranscript;

  const releaseStream = useCallback(() => {
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
  }, []);

  // On unmount, stop any in-flight recording and release the mic — otherwise
  // navigating away mid-recording leaves the microphone live.
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (recorderRef.current && recorderRef.current.state !== "inactive") {
        recorderRef.current.stop();
      }
      recorderRef.current = null;
      releaseStream();
    };
  }, [releaseStream]);

  const start = useCallback(async () => {
    if (recorderRef.current || startingRef.current) return; // already recording / starting
    startingRef.current = true;
    setError(null);
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      streamRef.current = stream;
      const recorder = new MediaRecorder(stream);
      chunksRef.current = [];

      recorder.ondataavailable = (e) => {
        if (e.data.size > 0) chunksRef.current.push(e.data);
      };
      recorder.onstop = async () => {
        releaseStream();
        const blob = new Blob(chunksRef.current, {
          type: recorder.mimeType || "audio/webm",
        });
        chunksRef.current = [];
        if (!mountedRef.current) return; // unmounted mid-recording — nothing to update
        if (blob.size === 0) {
          setState("idle");
          return;
        }
        setState("transcribing");
        try {
          const text = (await transcribeAudio(await blobToBase64(blob))).trim();
          if (!mountedRef.current) return;
          if (text) onTranscriptRef.current(text);
          else setError("Didn't catch that — try recording again.");
        } catch (e) {
          if (mountedRef.current) setError(`Could not transcribe: ${String(e)}`);
        } finally {
          if (mountedRef.current) setState("idle");
        }
      };

      recorder.start();
      recorderRef.current = recorder;
      setState("recording");
    } catch (e) {
      releaseStream();
      recorderRef.current = null;
      setState("idle");
      setError(micErrorMessage(e));
    } finally {
      startingRef.current = false;
    }
  }, [releaseStream]);

  const stop = useCallback(() => {
    const recorder = recorderRef.current;
    recorderRef.current = null;
    if (recorder && recorder.state !== "inactive") {
      recorder.stop(); // fires onstop → transcribe
    } else {
      releaseStream();
      setState("idle");
    }
  }, [releaseStream]);

  return { state, error, start, stop };
}

/** Strip the `data:…;base64,` prefix a FileReader adds, leaving raw base64. */
function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onloadend = () => {
      const result = String(reader.result);
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.onerror = () => reject(reader.error ?? new Error("could not read the recording"));
    reader.readAsDataURL(blob);
  });
}

/** Turn a getUserMedia rejection into something a person can act on. */
function micErrorMessage(e: unknown): string {
  const name = (e as { name?: string })?.name;
  if (name === "NotAllowedError" || name === "SecurityError")
    return "Microphone access was blocked. Allow PM to use the microphone, then try again.";
  if (name === "NotFoundError" || name === "DevicesNotFoundError")
    return "No microphone was found.";
  return `Could not start recording: ${String(e)}`;
}
