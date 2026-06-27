// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Progress } from "pm";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 18, width: 320 };

export const Determinate = () => (
  <div style={col}>
    <Progress value={0.25} label="Embedding" />
    <Progress value={0.6} label="Embedding" />
    <Progress value={0.92} label="Embedding" />
  </div>
);

export const Indeterminate = () => (
  <div style={col}>
    <Progress label="Indexing vault" />
  </div>
);
