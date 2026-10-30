// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The description's link panel (#884, C19 §7.4): a link's words and address, as Google Calendar's
// Link button asks for them. An inline panel under the toolbar rather than a dialog, as the time-zone
// panel is: Escape closes only it. The address is read as typed (`linkFromTyped`): "example.com" is
// https://example.com, "name@example.com" a mail link, and anything else is refused here, before it
// reaches the description. The address goes only to the editor's link command, never into the page
// as an attribute.

import { useState } from "react";
import { linkFromTyped } from "../../../lib/calendarEdit/descriptionHtml";
import { Button, Input, useFieldA11y } from "../../ui";

/** Said under Link when what was typed isn't a web or email address. */
export const NOT_AN_ADDRESS = "Type a web address, like example.com, or an email address.";

interface Props {
  /** The selection's words, or the link's. */
  text: string;
  /** The link's address, or "". */
  href: string;
  /** The caret is in a link: Remove link is offered. */
  editing: boolean;
  /** The words (possibly "", for the address as its own words) and the address read as typed. */
  onApply: (text: string, href: string) => void;
  onRemove: () => void;
  onCancel: () => void;
}

export function LinkPanel({ text, href, editing, onApply, onRemove, onCancel }: Props) {
  const [words, setWords] = useState(text);
  const [address, setAddress] = useState(href);
  const [refused, setRefused] = useState(false);
  const textField = useFieldA11y();
  const linkField = useFieldA11y({ error: refused ? NOT_AN_ADDRESS : undefined });

  const apply = () => {
    const url = linkFromTyped(address);
    if (url === null) {
      setRefused(true);
      return;
    }
    onApply(words, url);
  };

  return (
    <div
      className="flex flex-col gap-2 rounded-[var(--radius-sm)] border border-border2 p-2"
      onKeyDown={(e) => {
        // Escape closes this panel, not the whole editor; Enter in either field applies.
        if (e.key === "Escape") {
          e.stopPropagation();
          onCancel();
        } else if (e.key === "Enter" && e.target instanceof HTMLInputElement) {
          e.preventDefault();
          apply();
        }
      }}
    >
      <div className="flex flex-col gap-1">
        <label {...textField.labelProps} className="text-xs text-ink2">
          Text
        </label>
        <Input
          {...textField.controlProps}
          value={words}
          onChange={(e) => setWords(e.target.value)}
        />
      </div>
      <div className="flex flex-col gap-1">
        <label {...linkField.labelProps} className="text-xs text-ink2">
          Link
        </label>
        <Input
          {...linkField.controlProps}
          // The address is what the panel is for: it takes focus first.
          autoFocus
          value={address}
          onChange={(e) => {
            setAddress(e.target.value);
            setRefused(false);
          }}
        />
        {refused && (
          <p {...linkField.errorProps} className="text-xs text-st-due">
            {NOT_AN_ADDRESS}
          </p>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {/* Secondary: the editor's Save is the one primary button on screen. */}
        <Button variant="secondary" size="xs" onClick={apply}>
          Apply
        </Button>
        {editing && (
          <Button variant="tertiary" size="xs" onClick={onRemove}>
            Remove link
          </Button>
        )}
        <Button variant="tertiary" size="xs" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
}
