// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Button } from "pm";

const row: React.CSSProperties = {
  display: "flex",
  gap: 10,
  alignItems: "center",
  flexWrap: "wrap",
};

export const Variants = () => (
  <div style={row}>
    <Button variant="primary">Save changes</Button>
    <Button variant="secondary">Add source</Button>
    <Button variant="tertiary">Cancel</Button>
  </div>
);

export const Disabled = () => (
  <div style={row}>
    <Button variant="primary" disabled>
      Save changes
    </Button>
    <Button variant="secondary" disabled>
      Add source
    </Button>
    <Button variant="tertiary" disabled>
      Cancel
    </Button>
  </div>
);

export const InContext = () => (
  <div style={{ display: "flex", justifyContent: "flex-end", gap: 8, width: 320 }}>
    <Button variant="tertiary">Cancel</Button>
    <Button variant="primary">Rebuild index</Button>
  </div>
);
