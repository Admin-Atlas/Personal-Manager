// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";
import { SegmentedControl } from "pm";

const DEPTH = [
  { value: "min", label: "Minimal" },
  { value: "standard", label: "Standard" },
  { value: "power", label: "Power" },
];

const IMPORTANCE = [
  { value: "low", label: "Low" },
  { value: "normal", label: "Normal" },
  { value: "high", label: "High" },
];

export const Depth = () => {
  const [v, setV] = useState("standard");
  return <SegmentedControl options={DEPTH} value={v} onChange={setV} />;
};

export const Importance = () => {
  const [v, setV] = useState("high");
  return <SegmentedControl options={IMPORTANCE} value={v} onChange={setV} />;
};
