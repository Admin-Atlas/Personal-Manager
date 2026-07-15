// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The token-driven primitive set. Surfaces import these instead of hand-rolling Tailwind on each
// element, so a design change lands once, here. See AGENTS.md "Design system".

export { cn } from "./cn";
export { Button, type ButtonProps, type ButtonVariant } from "./Button";
export { Input } from "./Input";
export { Textarea } from "./Textarea";
export { Select } from "./Select";
export { SegmentedControl, type SegOption, type SegmentedControlProps } from "./SegmentedControl";
export { Card, type CardProps } from "./Card";
export { Collapsible, type CollapsibleProps } from "./Collapsible";
export { SectionInfo, type SectionInfoProps } from "./SectionInfo";
export { ListRow, type ListRowProps } from "./ListRow";
export { StatusBadge, STATUS_LABEL, type StatusBadgeProps } from "./StatusBadge";
export { Modal, type ModalProps } from "./Modal";
export { Toggle, type ToggleProps } from "./Toggle";
export { Tooltip, type TooltipProps } from "./Tooltip";
export { ConfirmDialog, type ConfirmDialogProps } from "./ConfirmDialog";
export { NavItem, type NavItemProps } from "./NavItem";
export { Progress, type ProgressProps } from "./Progress";
export { HScroll } from "./HScroll";
export { Skeleton, type SkeletonProps } from "./Skeleton";
export { TitleBar } from "./TitleBar";
