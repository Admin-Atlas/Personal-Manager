// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The line under a description when a mail link didn't open (C19b, critique C2-M2).

import { describe, expect, it } from "vitest";
import { MAIL_APP_FAILED, mailFailureText } from "./useMailLink";

describe("mailFailureText", () => {
  it("names the address when the mail app didn't open", () => {
    expect(mailFailureText(MAIL_APP_FAILED, "mailto:ann@example.com?subject=Hi#x")).toBe(
      "PM couldn't open your mail app. The address is ann@example.com.",
    );
    expect(mailFailureText(MAIL_APP_FAILED, "mailto:a@example.com,b@example.org#top")).toBe(
      "PM couldn't open your mail app. The address is a@example.com,b@example.org.",
    );
  });

  it("says the backend's own words for a link it refused", () => {
    expect(mailFailureText("That doesn't look like an email address.", "mailto:a%0Ab")).toBe(
      "That doesn't look like an email address.",
    );
  });

  // Anything but the backend's sentence (a call that never reached it) still gives the address.
  it("names the address for a failure with no words", () => {
    expect(mailFailureText(new Error("x"), "mailto:ann@example.com")).toBe(
      "PM couldn't open your mail app. The address is ann@example.com.",
    );
  });

  // reader.rs pins its side: `a_launch_that_failed_says_which_app`.
  it("matches the backend's sentence", () => {
    expect(MAIL_APP_FAILED).toBe("PM couldn't open your mail app.");
  });
});
