// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { ModelPicker } from "./ModelPicker";

interface Props {
  label: string;
  description?: string;
  /** Ordered preferred models (first = primary, the rest are fallbacks). */
  models: string[];
  onChange: (models: string[]) => void;
  autoSwitch: boolean;
  onAutoSwitchChange: (enabled: boolean) => void;
  helpId?: string;
}

/**
 * Edits one role's ordered list of preferred models plus its auto-switch toggle.
 * The first model is the primary; when auto-switch is on, the rest act as
 * fallbacks the backend hands to OpenRouter so it advances to the next model when
 * the active one hits its limit. Reuses `ModelPicker` as the "add a model" control.
 */
export function ModelListEditor({
  label,
  description,
  models,
  onChange,
  autoSwitch,
  onAutoSwitchChange,
  helpId,
}: Props) {
  function add(id: string) {
    const trimmed = id.trim();
    if (!trimmed || models.includes(trimmed)) return;
    onChange([...models, trimmed]);
  }
  function remove(id: string) {
    onChange(models.filter((m) => m !== id));
  }
  function move(i: number, dir: -1 | 1) {
    const j = i + dir;
    if (j < 0 || j >= models.length) return;
    const next = [...models];
    [next[i], next[j]] = [next[j], next[i]];
    onChange(next);
  }

  return (
    <div data-help={helpId}>
      <label className="block text-sm font-medium text-neutral-300">{label}</label>
      {description && <p className="mt-0.5 text-xs text-neutral-500">{description}</p>}

      <div className="mt-2 space-y-1.5">
        {models.length === 0 && (
          <p className="rounded-lg border border-dashed border-neutral-800 px-3 py-2 text-xs text-neutral-500">
            No model chosen — the default will be used.
          </p>
        )}
        {models.map((id, i) => (
          <div
            key={id}
            className="flex items-center gap-2 rounded-lg border border-neutral-800 bg-neutral-950 px-2.5 py-1.5"
          >
            <span className="min-w-0 flex-1 truncate text-sm text-neutral-200" title={id}>
              {id}
            </span>
            {i === 0 ? (
              <span className="shrink-0 rounded bg-emerald-500/15 px-1.5 py-0.5 text-[10px] font-medium text-emerald-300">
                Primary
              </span>
            ) : (
              <span className="shrink-0 text-[10px] text-neutral-600">fallback {i}</span>
            )}
            <div className="flex shrink-0 items-center text-neutral-500">
              <button
                type="button"
                onClick={() => move(i, -1)}
                disabled={i === 0}
                title="Move up"
                className="rounded px-1.5 py-0.5 hover:bg-neutral-800 hover:text-neutral-200 disabled:opacity-30"
              >
                ↑
              </button>
              <button
                type="button"
                onClick={() => move(i, 1)}
                disabled={i === models.length - 1}
                title="Move down"
                className="rounded px-1.5 py-0.5 hover:bg-neutral-800 hover:text-neutral-200 disabled:opacity-30"
              >
                ↓
              </button>
              <button
                type="button"
                onClick={() => remove(id)}
                title="Remove"
                className="rounded px-1.5 py-0.5 hover:bg-red-950/60 hover:text-red-300"
              >
                ×
              </button>
            </div>
          </div>
        ))}
      </div>

      {/* `value=""` keeps the picker as an "add" control — each pick appends. */}
      <ModelPicker value="" triggerLabel="Add a model…" onChange={add} />

      <div className="mt-2 flex items-start justify-between gap-3">
        <div>
          <span className="text-xs font-medium text-neutral-300">Auto-switch on limit</span>
          <p className="text-[11px] text-neutral-500">
            When the active model hits its limit, automatically fall through to the next in the
            list. Add a second model to use it.
          </p>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={autoSwitch}
          onClick={() => onAutoSwitchChange(!autoSwitch)}
          className={`mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
            autoSwitch ? "bg-amber-500" : "bg-neutral-700"
          }`}
        >
          <span
            className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${
              autoSwitch ? "translate-x-4" : "translate-x-0.5"
            }`}
          />
        </button>
      </div>
    </div>
  );
}
