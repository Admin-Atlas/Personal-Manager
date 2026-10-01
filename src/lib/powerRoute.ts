// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The words for the On battery policy (#432), as pure readings of the status snapshot.
//
// Everything that DECIDES lives in Rust: the threshold, the return band, the one-minute guard, which
// role can move and why not. The backend computes `power` with the same functions routing uses, so
// the status cannot say one thing while the gateway does another. This file only words what it was
// handed — it never re-derives a level or a time, because a second copy of that arithmetic is
// exactly how a readout ends up promising a switch the router will not make.
//
// One wording rule runs through all of it: the cloud route REDUCES power use. It does not stop the
// battery draining — the card stays awake while the server holds a model — so `REDUCES_POWER` is
// the only way any of this copy says what the switch buys, and a test holds the rest of it to that.

import type { LocalRole } from "./localModelState";
import type { LocalLlmStatus, PowerBlocked, PowerRoleView, PowerScope, PowerView } from "./types";

/** The locked phrase for what moving to the cloud buys. Never "stops draining your battery". */
export const REDUCES_POWER = "reduces power use by not running inference on your GPU";

/** The levels the switch is offered at, 0 being "Never". The backend clamps to 0..=80. */
export const THRESHOLD_OPTIONS = [0, 20, 30, 40, 50, 60, 70, 80] as const;

/** Typed fixture for the four LocalLlmStatus test factories; also the shape a missing power object
 *  would be read as. Nothing in it can move a role or ask a question. */
export const INERT_POWER_VIEW: PowerView = {
  source: "unknown",
  percent: null,
  has_battery: false,
  state: "mains",
  threshold: 60,
  return_at: 75,
  roles: "both",
  consent: null,
  consent_needed: false,
  keep_local: false,
  any_cloud_key: false,
  chat: { route: "unchanged", blocked: "no_local_model", local_model: null },
  background: { route: "unchanged", blocked: "no_local_model", local_model: null },
};

/** The two roles, in the order every sentence names them. */
const ROLES: readonly LocalRole[] = ["chat", "background"];

/** A snapshot's power half. A backend older than the policy sends none, and that reads as inert
 *  rather than as a crash on `status.power.chat`. */
export function powerOf(status: LocalLlmStatus): PowerView {
  return status.power ?? INERT_POWER_VIEW;
}

function roleView(power: PowerView, role: LocalRole): PowerRoleView {
  return role === "chat" ? power.chat : power.background;
}

/** The roles a scope choice covers. */
function covered(scope: PowerScope): LocalRole[] {
  if (scope === "chat") return ["chat"];
  if (scope === "background") return ["background"];
  return ["chat", "background"];
}

/** The quiet mark a footer row or the composer wears for its role. */
export type PowerTag = "on_battery" | "kept_local";

/** null unless status && status.configured && route is "cloud" | "kept_local" — the same zero-pixel
 *  contract `localModelActivity` keeps. "needs_consent" deliberately shows nothing here: the strip
 *  owns it, and the local model really is still answering. */
export function powerTag(status: LocalLlmStatus | null, role: LocalRole): PowerTag | null {
  if (!status || !status.configured) return null;
  const route = roleView(powerOf(status), role).route;
  if (route === "cloud") return "on_battery";
  if (route === "kept_local") return "kept_local";
  return null;
}

/** Whether the On battery section can do anything here, and if not, which explanation it owes. */
export type PowerGate =
  | "cloud_only"
  | "loading"
  | "no_key"
  | "key_unreadable"
  | "not_with_roles"
  | "no_battery"
  | "ready";

/** Evaluated in order: the first answer that applies is the one the section shows. */
export function powerGate(
  configured: boolean,
  anyLocalRoleWithModel: boolean,
  status: LocalLlmStatus | null,
): PowerGate {
  if (!configured || !anyLocalRoleWithModel) return "cloud_only";
  if (!status) return "loading";
  const power = powerOf(status);
  // A machine that really has no battery first: on a desktop nothing here can ever apply, and
  // explaining which key or role would "make this available" would promise something that won't
  // happen. Only a positive AC reading counts — an unreadable sample is not evidence of a desktop.
  if (!power.has_battery && power.source === "ac") return "no_battery";
  const blocked = [power.chat.blocked, power.background.blocked];
  if (blocked.every((b) => b !== null)) {
    // Unreadable first: "no key" must never be said about keys PM simply couldn't read.
    if (blocked.includes("key_unreadable")) return "key_unreadable";
    // The keyless copy says no cloud provider is set up, which a background-only key makes false.
    if (blocked.includes("no_key") && !power.any_cloud_key) return "no_key";
    return "not_with_roles";
  }
  return "ready";
}

/** "chat" | "background work" | "chat and background work". One role takes "is", both take "are". */
export function whoPhrase(chat: boolean, background: boolean): string {
  if (chat && background) return "chat and background work";
  if (chat) return "chat";
  if (background) return "background work";
  return "nothing";
}

function who(roles: readonly LocalRole[]): string {
  return whoPhrase(roles.includes("chat"), roles.includes("background"));
}

/** What the consent question names as the thing that would leave the machine. */
export function askPhrase(chat: boolean, background: boolean): string {
  if (chat && background) return "your chats and background work";
  // Named for what it reads: titles, summaries and learning are written from the user's chats, so
  // a yes to "background work" must not sound like it keeps every word of a conversation local.
  if (background)
    return "background work (filing, and the titles, summaries and learning PM writes from your chats)";
  return "your chats";
}

/** The pronoun for `askPhrase`: "them" for chats or both, "it" for background work alone. */
export function themPhrase(chat: boolean, background: boolean): string {
  return background && !chat ? "it" : "them";
}

/** The scope a set of roles is, for writing a consent. */
export function scopeOf(chat: boolean, background: boolean): PowerScope | null {
  if (chat && background) return "both";
  if (chat) return "chat";
  if (background) return "background";
  return null;
}

/** " at 58%", or nothing when PM has no percentage to quote. */
function atPercent(percent: number | null): string {
  return percent == null ? "" : ` at ${percent}%`;
}

const ROLE_NAME: Record<LocalRole, string> = { chat: "Chat", background: "Background work" };

/** Why one role can't be moved, as a clause with no full stop (callers punctuate). */
export function blockedReason(role: LocalRole, blocked: PowerBlocked, power: PowerView): string {
  const name = ROLE_NAME[role];
  switch (blocked) {
    case "cloud_routing":
      return `${name} already uses the cloud`;
    case "local_only":
      return `${name} is set to Local only, which never uses the cloud`;
    case "no_local_model":
      return `${name} has no local model`;
    case "no_key":
      // The one asymmetric key rule: background may use its own key, chat only the main one. A
      // background-key-only setup reads as "no key" for chat, and saying just that would be a lie
      // about a key the user can see in Settings.
      return role === "chat" && power.any_cloud_key
        ? "Chat uses your main OpenRouter key, and only a background key is set up"
        : `${name} has no OpenRouter key to use`;
    case "key_unreadable":
      return "PM can't read your saved keys right now";
  }
}

/** The reasons the given roles can't move, deduplicated (two unreadable keys are one sentence). */
function reasonsFor(roles: readonly LocalRole[], power: PowerView): string[] {
  const out: string[] = [];
  for (const role of roles) {
    const blocked = roleView(power, role).blocked;
    if (blocked === null) continue;
    const reason = blockedReason(role, blocked, power);
    if (!out.includes(reason)) out.push(reason);
  }
  return out;
}

/** The "with your setup" note under one What moves option: what that choice would really move. */
export function scopeNote(scope: PowerScope, power: PowerView): string {
  const roles = covered(scope);
  const moving = roles.filter((r) => roleView(power, r).blocked === null);
  const reasons = reasonsFor(roles, power).join("; ");
  if (moving.length === roles.length) return `With your setup this moves ${who(moving)}.`;
  if (moving.length === 0) return `With your setup this moves nothing: ${reasons}.`;
  return `With your setup only ${who(moving)} would move: ${reasons}.`;
}

/** The roles the chosen scope covers that nothing else stops from moving. */
function movableNow(power: PowerView): LocalRole[] {
  return covered(power.roles).filter((r) => roleView(power, r).blocked === null);
}

/** ", after asking you once" when some of those roles haven't been agreed to yet. */
function askingFirst(power: PowerView, roles: readonly LocalRole[]): string {
  const agreed = power.consent == null ? [] : covered(power.consent);
  return roles.every((r) => agreed.includes(r)) ? "" : ", after asking you once";
}

/** The consent line under the section's controls. */
export function consentLine(power: PowerView): string {
  if (power.consent == null) {
    return "PM asks you first, the first time it would move anything to the cloud, and stays on your local model until you answer. Your answer covers what it asked about; if something else could move later, PM asks about that once too.";
  }
  const agreed = covered(power.consent);
  return `You've allowed PM to use the cloud on battery for ${who(agreed)}.`;
}

/** The section's live readout, first match wins. The consent buttons are not part of it — the
 *  section renders those beside it when `consent_needed`. `loaded*` are the status's `*_loaded`. */
export function powerReadout(
  power: PowerView,
  loadedChat: boolean | null,
  loadedBackground: boolean | null,
): string {
  const t = power.threshold;
  const at = atPercent(power.percent);
  const plugged = `Plugged in${power.percent == null ? "" : `, battery at ${power.percent}%`}.`;

  // Only while PM really is acting as though it's plugged in. An unreadable sample doesn't undo a
  // settled battery state at once — it has to last the same minute as any other change — and until
  // then the rows below describe what PM is actually doing.
  if (power.source === "unknown" && power.state === "mains") {
    return "PM can't tell right now whether you're plugged in, so it's acting as though you are.";
  }
  if (power.state !== "mains" && power.source === "ac") {
    // "Moves back" only if something actually moved. Otherwise nothing ever left the machine, and
    // the minute the latch is waiting out changes nothing anyone would notice.
    return ROLES.some((r) => roleView(power, r).route === "cloud")
      ? "Plugged in. PM moves back to your local model in about a minute."
      : "Plugged in. PM is using your local model.";
  }
  if (power.state === "mains" && power.source === "battery") {
    return `On battery${at}. PM waits a minute before acting on a change, so unplugging to move the laptop changes nothing.`;
  }
  if (power.state === "mains") {
    if (t === 0) {
      return `${plugged} PM uses your local model as normal, and switching on battery is off.`;
    }
    const movable = movableNow(power);
    if (movable.length === 0) {
      return `${plugged} With what you've chosen below, nothing would move on battery.`;
    }
    if (power.keep_local) {
      return `${plugged} You've asked PM to keep using your local model until you quit, so nothing moves on battery.`;
    }
    return `${plugged} On battery at ${t}% or below, PM moves ${who(movable)} to the cloud${askingFirst(power, movable)}.`;
  }
  if (power.state === "battery") {
    if (t === 0) return `On battery${at}. Switching is off, so PM keeps using your local model.`;
    const movable = movableNow(power);
    if (movable.length === 0) {
      return `On battery${at}. With what you've chosen below, nothing moves, so PM keeps using your local model.`;
    }
    if (power.keep_local) {
      return `On battery${at}. You asked PM to keep using your local model until you quit, so nothing moves.`;
    }
    // A battery that won't say its charge is never "low" (Rust's rule), so there is no level to wait
    // for. And at or below the level, the move is a settle away, not a further drop away.
    if (power.percent == null) {
      return "On battery. Your battery isn't reporting its charge, so PM keeps using your local model.";
    }
    if (power.percent <= t) {
      return `On battery${at}. Once that has lasted a minute, PM moves ${who(movable)} to the cloud${askingFirst(power, movable)}.`;
    }
    return `On battery${at}. PM keeps using your local model until the battery falls to ${t}%, then moves ${who(movable)} to the cloud${askingFirst(power, movable)}.`;
  }

  // battery_low
  const routed = (route: PowerRoleView["route"]) =>
    ROLES.filter((r) => roleView(power, r).route === route);
  const cloud = routed("cloud");
  if (cloud.length > 0) {
    let text = `On battery${at}, so ${who(cloud)} ${cloud.length > 1 ? "are" : "is"} going to the cloud. That ${REDUCES_POWER}. PM moves back to your local model about a minute after you plug in, or if the battery climbs back to ${power.return_at}%.`;
    // A parked model still on the card is the part of the saving the switch alone doesn't get, and
    // the one place this readout can point at the setting that does.
    const loaded: string[] = [];
    for (const role of cloud) {
      const model = roleView(power, role).local_model;
      const isLoaded = role === "chat" ? loadedChat : loadedBackground;
      if (isLoaded === true && model && !loaded.includes(model)) loaded.push(model);
    }
    if (loaded.length > 0) {
      const many = loaded.length > 1;
      text += ` ${loaded.join(" and ")} ${many ? "are" : "is"} still loaded on your graphics card, and a loaded model keeps the card drawing power. To hand ${many ? "them" : "it"} back on battery, choose a time for "On battery, hand the memory back" under Holding the graphics card.`;
    }
    return text;
  }
  if (routed("needs_consent").length > 0) {
    return `On battery${at}. PM is waiting for your go-ahead before it uses the cloud, and is staying on your local model until then.`;
  }
  if (routed("kept_local").length > 0) {
    return `On battery${at}. You asked PM to keep using your local model until you quit, so nothing has moved.`;
  }
  return `On battery${at}. Nothing you've chosen to move can use the cloud with your current setup, so PM is staying on your local model.`;
}

/** The consent question's first paragraph, naming only the roles that are actually waiting on it. */
export function consentText(power: PowerView): string {
  const chat = power.chat.route === "needs_consent";
  const background = power.background.route === "needs_consent";
  return `You're on battery${atPercent(power.percent)}. To reduce power use, PM can send ${askPhrase(chat, background)} to your OpenRouter cloud model instead of running ${themPhrase(chat, background)} on your graphics card, until you plug in. While it does, what you send leaves this machine and is billed to your OpenRouter key at your cloud model's usual rates. Until you answer, PM keeps using your local model.`;
}

/** The sidebar's "on battery" chip, spelled out. */
export const POWER_DETAIL = `On battery, so PM is sending this to the cloud. That ${REDUCES_POWER}. It goes back to your local model about a minute after you plug in.`;

/** A power-routed footer row's title: the cloud model answering, and the local one parked. */
export function onBatteryRowTitle(cloud: string | null, parked: string | null): string {
  return `${cloud ?? "your cloud model"} — on the cloud while you're on battery${parked ? `; ${parked} is waiting on this machine` : ""}`;
}

/** The sidebar's "kept local" chip, spelled out. */
export function keptLocalTitle(local: string | null): string {
  return `On battery, but you asked PM to keep using ${local ?? "your local model"} until you quit.`;
}

/** The composer chip's title while chat is power-routed. */
export const COMPOSER_POWER_TITLE = `On battery, so chat is going to your cloud model. That ${REDUCES_POWER}. Manage it in Settings → Local AI → On battery.`;
