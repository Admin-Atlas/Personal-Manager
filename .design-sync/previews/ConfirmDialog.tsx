// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { ConfirmDialog } from "pm";

// Sized, transformed stage so the overlay's position:fixed is contained and the dialog centers in
// the card (see Modal.tsx).
const stage: React.CSSProperties = {
  position: "relative",
  transform: "translateZ(0)",
  width: "100%",
  height: 300,
  overflow: "hidden",
  borderRadius: 8,
};

export const Destructive = () => (
  <div style={stage}>
    <ConfirmDialog
      open
      danger
      title="Rebuild the search index?"
      confirmLabel="Rebuild"
      onConfirm={() => {}}
      onClose={() => {}}
    >
      This re-embeds every document in the vault. It can take several minutes and the assistant
      can't answer questions until it finishes.
    </ConfirmDialog>
  </div>
);

export const Confirm = () => (
  <div style={stage}>
    <ConfirmDialog
      open
      title="Disconnect this account?"
      confirmLabel="Disconnect"
      onConfirm={() => {}}
      onClose={() => {}}
    >
      The local index for user@email.com will be removed. You can reconnect any time.
    </ConfirmDialog>
  </div>
);
