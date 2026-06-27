// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Modal, Button } from "pm";

// The overlay's position:fixed is contained by the nearest transformed ancestor. This sized,
// transformed stage makes the scrim fill the card and the dialog center inside it (instead of
// pinning to a zero-height box and cropping at the top).
const stage: React.CSSProperties = {
  position: "relative",
  transform: "translateZ(0)",
  width: "100%",
  height: 300,
  overflow: "hidden",
  borderRadius: 8,
};

export const Dialog = () => (
  <div style={stage}>
    <Modal open onClose={() => {}}>
      <div style={{ padding: 20 }}>
        <h2 className="font-head text-ink" style={{ fontSize: 17, fontWeight: 600, margin: 0 }}>
          Connect Google Drive
        </h2>
        <p className="text-ink3" style={{ fontSize: 13, lineHeight: 1.6, marginTop: 8 }}>
          PM will index the folders you choose so you can search and chat over them. Files stay in
          your Drive — only an encrypted index is stored locally.
        </p>
        <div style={{ display: "flex", justifyContent: "flex-end", gap: 8, marginTop: 20 }}>
          <Button variant="tertiary">Not now</Button>
          <Button variant="primary">Choose folders</Button>
        </div>
      </div>
    </Modal>
  </div>
);
