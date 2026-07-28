// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import {
  checkLocalLlmEndpoint,
  probeLocalLlmPorts,
  setLocalLlmEndpoint,
  setLocalLlmRoleModel,
  setLocalLlmRouting,
} from "../../lib/ipc";
import { Button, Input, Select } from "../ui";

/**
 * The "On this device" onboarding pane: connect a local OpenAI-compatible server (Ollama / LM
 * Studio / llama-server) and pick a model, so a user can start PM with no cloud key (#295). It
 * composes the existing local-AI IPC — nothing new backend-side — and reports readiness up via
 * `onConfigured` (true once an endpoint + a chat model are set and routing is pointed local). The
 * full controls live in Settings → Local AI afterwards; this is just enough to get started.
 */
export function OnboardingLocalConnect({
  onConfigured,
}: {
  onConfigured: (ready: boolean) => void;
}) {
  const [url, setUrl] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [models, setModels] = useState<string[] | null>(null);
  // The endpoint serves models, but every one of them is an embedder — a distinct state from
  // "no models at all", and the user needs a different instruction.
  const [embeddersOnly, setEmbeddersOnly] = useState(false);
  const [model, setModel] = useState("");
  const [error, setError] = useState<string | null>(null);

  // Auto-detect a running server on mount so the common case is one click.
  useEffect(() => {
    void (async () => {
      try {
        const found = await probeLocalLlmPorts();
        if (found.length > 0) setUrl(found[0].url);
      } catch {
        /* ignore — the user can type the address */
      }
    })();
  }, []);

  function resetConnection() {
    setModels(null);
    setModel("");
    setEmbeddersOnly(false);
    onConfigured(false);
  }

  async function pick(m: string) {
    setModel(m);
    setError(null);
    try {
      await setLocalLlmRoleModel("chat", m);
      await setLocalLlmRoleModel("background", m);
      await setLocalLlmRouting("chat", "local");
      await setLocalLlmRouting("background", "local");
      onConfigured(true);
    } catch (e) {
      setError(String(e));
      onConfigured(false);
    }
  }

  async function connect() {
    setConnecting(true);
    setError(null);
    resetConnection();
    try {
      const check = await checkLocalLlmEndpoint(url);
      if (check.scheme_verdict === "refused_public_cleartext") {
        setError(
          "That's a public http address — PM won't send your data to it in the clear. Use a server on this machine or your private network.",
        );
        return;
      }
      if (!check.reachable) {
        setError("Couldn't reach that address — is your local model server running?");
        return;
      }
      const saved = await setLocalLlmEndpoint(url);
      setUrl(saved);
      // Offer — and auto-pick from — only what can actually answer a chat. `models` is everything
      // the server serves, and Ollama/LM Studio serve embedding models on the same endpoint; binding
      // one to both roles here would leave a first-run user with no cloud key silently unable to
      // chat. `assignable` is that list minus the embedders.
      setModels(check.assignable);
      setEmbeddersOnly(check.assignable.length === 0 && check.models.length > 0);
      if (check.assignable.length > 0) await pick(check.assignable[0]);
    } catch (e) {
      setError(String(e));
    } finally {
      setConnecting(false);
    }
  }

  return (
    <div className="mt-3 space-y-3">
      <p className="text-xs leading-relaxed text-ink4">
        Run a model on your own machine with{" "}
        <a
          href="https://ollama.com"
          target="_blank"
          rel="noreferrer"
          className="text-accent-text underline hover:brightness-110"
        >
          Ollama
        </a>
        , LM Studio, or any OpenAI-compatible server, then point PM at it. Nothing leaves your
        device. You can fine-tune this later in Settings → Local AI.
      </p>
      <div className="flex items-center gap-2">
        <Input
          value={url}
          onChange={(e) => {
            setUrl(e.target.value);
            resetConnection();
          }}
          placeholder="http://localhost:11434"
          className="flex-1"
          data-help="onboarding-local-url"
        />
        <Button
          variant="secondary"
          onClick={() => void connect()}
          disabled={connecting || !url.trim()}
        >
          {connecting ? "Connecting…" : "Connect"}
        </Button>
      </div>

      {models !== null &&
        (models.length > 0 ? (
          <label className="block text-xs text-ink3">
            Model
            <Select
              value={model}
              onChange={(e) => void pick(e.target.value)}
              className="mt-1 w-full"
            >
              {models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </Select>
          </label>
        ) : embeddersOnly ? (
          <p className="text-xs text-st-look">
            Connected, but the only models that server has are embedding models — they turn text
            into numbers for search and can't hold a conversation. Pull a chat model (e.g.{" "}
            <code>ollama pull llama3</code>) and press Connect again.
          </p>
        ) : (
          <p className="text-xs text-st-look">
            Connected, but that server has no models yet — pull one (e.g.{" "}
            <code>ollama pull llama3</code>) and press Connect again.
          </p>
        ))}

      {error && <p className="text-xs text-st-due">{error}</p>}
    </div>
  );
}
