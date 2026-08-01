// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The token-driven primitive set. Surfaces import these instead of hand-rolling Tailwind on each
// element, so a design change lands once, here. See AGENTS.md "Design system".

export { cn } from "./cn";
export { Button, type ButtonProps, type ButtonVariant, type ButtonSize } from "./Button";
export {
  Callout,
  type CalloutProps,
  type CalloutTone,
  type CalloutVariant,
  type CalloutSize,
  type CalloutBody,
} from "./Callout";
// The one tone→recipe map, shared by Callout, Button's danger variant and dialog chrome. Import it
// rather than writing a ratio anywhere else.
export { TONE_TOKEN, TONE_TEXT_TOKEN, TONE_MIX, toneMix, toneSurface, type Tone } from "./tone";
export { IconButton, type IconButtonProps, type IconButtonVariant } from "./IconButton";
export { Input } from "./Input";
export { Textarea } from "./Textarea";
export { Select } from "./Select";
export { SegmentedControl, type SegOption, type SegmentedControlProps } from "./SegmentedControl";
// The settings-markup pair. A section's heading, and one row of it — between them they own the
// class strings that had been retyped 27 and 40 times, and the label→control association that
// nothing supplied.
export { SectionLabel, type SectionLabelProps } from "./SectionLabel";
export { SettingRow, type SettingRowProps } from "./SettingRow";
export { Card, type CardProps } from "./Card";
export { Collapsible, type CollapsibleProps } from "./Collapsible";
export { SectionInfo, type SectionInfoProps } from "./SectionInfo";
export { StatusBadge, STATUS_LABEL, type StatusBadgeProps } from "./StatusBadge";
// Modal is the dialog SHELL (role, aria-modal, Escape, focus trap, scrim); Dialog is the chrome
// worn over it, and the reason a dialog cannot ship without an accessible name — its `title` is
// required and wired to `aria-labelledby` for you.
export {
  Modal,
  type ModalProps,
  type ModalBaseProps,
  type ModalNameProps,
  type ModalPlacement,
} from "./Modal";
export { Dialog, type DialogProps, type DialogChrome, type DialogTone } from "./Dialog";
export { Toggle, type ToggleProps } from "./Toggle";
export { Tooltip, type TooltipProps } from "./Tooltip";
export { ConfirmDialog, type ConfirmDialogProps } from "./ConfirmDialog";
export { NavItem, type NavItemProps } from "./NavItem";
export { Popover } from "./Popover";
export { Progress, type ProgressProps } from "./Progress";
export { HScroll } from "./HScroll";
export { Skeleton, type SkeletonProps } from "./Skeleton";
export { TitleBar } from "./TitleBar";
export { VisuallyHidden } from "./VisuallyHidden";
export { Field, useFieldA11y, type FieldProps, type FieldA11y } from "./Field";
export { ErrorBoundary, type ErrorBoundaryProps } from "./ErrorBoundary";
