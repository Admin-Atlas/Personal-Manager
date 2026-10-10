// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Whether a key press is the letter `letter` (lower case, "z") as the keyboard's layout labels it, for
 * a Ctrl/Cmd letter shortcut. The letter the layout types comes first: matching the physical key
 * (`code`) alone made Ctrl+Z redo on a German keyboard, whose Z sits where QWERTY has Y, and do
 * nothing on a French one. Only a layout that types no Latin letter there (Cyrillic, Greek) falls back
 * to the physical key, so the shortcut still works where its letter isn't on the keyboard.
 */
export function isLetterKey(e: { key: string; code: string }, letter: string): boolean {
  const typed = e.key.length === 1 ? e.key.toLowerCase() : "";
  if (/^[a-z]$/.test(typed)) return typed === letter;
  return e.code === `Key${letter.toUpperCase()}`;
}
