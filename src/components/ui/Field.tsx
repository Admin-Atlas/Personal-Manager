// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Programmatic label / description / error wiring for a form control (WCAG 1.3.1, 3.3.1, 3.3.2,
// 4.1.2). `useFieldA11y` mints ids with `useId` and returns the ARIA props to spread onto the label,
// the control, and the error node — so the control is named, and a validation error is both
// associated (`aria-describedby`) and announced (`role="alert"`). `Field` is the common vertical
// layout built on the hook; reach for the hook directly when a surface needs its own layout (e.g. the
// centered vault-unlock gate, which keeps a visually-hidden label so nothing changes on screen).

import { useId, type ReactNode } from "react";
import { cn } from "./cn";

export interface FieldA11y {
  labelProps: { htmlFor: string };
  controlProps: { id: string; "aria-invalid"?: true; "aria-describedby"?: string };
  errorProps: { id: string; role: "alert" };
  descriptionProps: { id: string };
}

export function useFieldA11y({
  error,
  description,
}: { error?: ReactNode; description?: ReactNode } = {}): FieldA11y {
  const base = useId();
  const id = `${base}-control`;
  const errorId = `${base}-error`;
  const descId = `${base}-desc`;
  const describedBy = [description ? descId : null, error ? errorId : null]
    .filter(Boolean)
    .join(" ");
  return {
    labelProps: { htmlFor: id },
    controlProps: {
      id,
      ...(error ? { "aria-invalid": true as const } : {}),
      ...(describedBy ? { "aria-describedby": describedBy } : {}),
    },
    errorProps: { id: errorId, role: "alert" },
    descriptionProps: { id: descId },
  };
}

export interface FieldProps {
  label: ReactNode;
  error?: ReactNode;
  description?: ReactNode;
  className?: string;
  /** Render the control, spreading the passed ARIA props (id + aria-invalid/-describedby) onto it. */
  children: (controlProps: FieldA11y["controlProps"]) => ReactNode;
}

export function Field({ label, error, description, className, children }: FieldProps) {
  const a11y = useFieldA11y({ error, description });
  return (
    <div className={cn("flex flex-col gap-1", className)}>
      <label {...a11y.labelProps} className="text-sm text-ink2">
        {label}
      </label>
      {description != null && (
        <p {...a11y.descriptionProps} className="text-xs text-ink4">
          {description}
        </p>
      )}
      {children(a11y.controlProps)}
      {error != null && error !== false && (
        <p {...a11y.errorProps} className="text-xs text-st-due">
          {error}
        </p>
      )}
    </div>
  );
}
