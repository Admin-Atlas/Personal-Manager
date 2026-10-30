// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The HTML sink fence (eslint.config.js's HTML_SINKS, C19 of #884; INVARIANTS.md I-23): every way of
// parsing HTML or writing it into the page fails the lint in shipped code, the calendar write hook
// included, and only tests may drive them. Linted with the repo's own config, without type
// information (the fence needs none), so the suite stays light.
//
// The file paths below are the repo's, read from where vitest runs (its root); ESLint finds
// eslint.config.js from each file's own path. No `node:` import: the frontend tsconfig carries no
// Node types, by design (designGuards.test.ts says why).

import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

const eslint = new ESLint({
  overrideConfig: [
    {
      files: ["src/**/*.{ts,tsx}"],
      languageOptions: { parserOptions: { projectService: false, project: null } },
      rules: {
        "@typescript-eslint/no-floating-promises": "off",
        "@typescript-eslint/no-misused-promises": "off",
      },
    },
  ],
});

const DECLARED = "declare const el: HTMLElement; declare const s: string;\n";

/** The fence's messages for `code` linted as the file at `filePath`. */
async function refusals(code: string, filePath: string): Promise<string[]> {
  const [result] = await eslint.lintText(DECLARED + code, { filePath });
  expect(result.messages.filter((m) => m.fatal)).toEqual([]);
  return result.messages.filter((m) => m.ruleId === "no-restricted-syntax").map((m) => m.message);
}

describe("the HTML sink fence", { timeout: 60_000 }, () => {
  it.each([
    "const parser = new DOMParser();",
    "new window.DOMParser();",
    '(el as unknown as DOMParser).parseFromString(s, "text/html");',
    "el.innerHTML = s;",
    "el.outerHTML = s;",
    'el["innerHTML"] = s;',
    "const read = el.innerHTML;",
    "Object.assign(el, { innerHTML: s });",
    'Object.assign(el, { "outerHTML": s });',
    'el.insertAdjacentHTML("beforeend", s);',
    'el["insertAdjacentHTML"]("beforeend", s);',
    "document.createRange().createContextualFragment(s);",
    "document.write(s);",
    "document.writeln(s);",
    "(el as unknown as { setHTMLUnsafe(x: string): void }).setHTMLUnsafe(s);",
    "(document.querySelector('iframe') as HTMLIFrameElement).srcdoc = s;",
    'el.setAttribute("srcdoc", s);',
    "document.createElement(s);",
    "document.createElement(`b`);",
    "document.execCommand(s);",
    'document.execCommand("insertHTML", false, s);',
  ])("refuses %j in shipped code", async (code) => {
    expect(await refusals(code, "src/lib/calendarEdit/descriptionHtml.ts")).not.toEqual([]);
  });

  it.each([
    "export const A = () => <div dangerouslySetInnerHTML={{ __html: s }} />;",
    "export const B = () => <iframe srcDoc={s} />;",
  ])("refuses %j in a component", async (code) => {
    expect(
      await refusals(code, "src/components/calendar/parts/CalendarEventPopover.tsx"),
    ).not.toEqual([]);
  });

  it("refuses them in the calendar write hook too", async () => {
    expect(
      await refusals("el.innerHTML = s;", "src/components/calendar/edit/useEventWrites.ts"),
    ).not.toEqual([]);
  });

  it("leaves what the codec and its views do alone", async () => {
    const code = [
      'const b = document.createElement("b");',
      "b.append(s);",
      'b.setAttribute("href", s);',
      "el.replaceChildren(b);",
      'document.execCommand("bold");',
      "const words = (el.firstChild as Text).data;",
    ].join("\n");
    expect(await refusals(code, "src/lib/calendarEdit/descriptionHtml.ts")).toEqual([]);
  });

  it("lets tests drive the sinks", async () => {
    const code = 'el.innerHTML = s; new DOMParser().parseFromString(s, "text/html");';
    expect(await refusals(code, "src/lib/calendarEdit/descriptionHtml.test.ts")).toEqual([]);
    expect(await refusals(code, "src/components/calendar/CalendarView.test.tsx")).toEqual([]);
  });
});
