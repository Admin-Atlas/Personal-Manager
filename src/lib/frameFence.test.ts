// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The mod-frame fence (eslint.config.js's FRAME_SINKS, NO_IFRAME_* and MOD_FRAME_SHAPE; INVARIANTS.md
// I-24): an <iframe> exists only in src/components/mods/ModFrame.tsx, as `sandbox="allow-scripts"
// allow=""` written literally, and nothing in the page makes a frame another way or touches a frame's
// sandbox. Linted through ESLint's own API with the repo's config, without type information (the fence
// needs none), like htmlSinkFence.test.ts.

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

const MOD_FRAME = "src/components/mods/ModFrame.tsx";
const ELSEWHERE = "src/components/calendar/parts/CalendarEventPopover.tsx";
const DECLARED =
  "declare const el: HTMLIFrameElement; declare const s: string; declare const p: object;\n";

/** The fence's messages for `code` linted as the file at `filePath`. */
async function refusals(code: string, filePath: string): Promise<string[]> {
  const [result] = await eslint.lintText(DECLARED + code, { filePath });
  expect(result.messages.filter((m) => m.fatal)).toEqual([]);
  return result.messages.filter((m) => m.ruleId === "no-restricted-syntax").map((m) => m.message);
}

/** ModFrame's frame with `attrs` in place of its usual ones. */
const frame = (attrs: string) => `export const F = () => <iframe ${attrs} />;`;
const SEALED = 'src={s} sandbox="allow-scripts" allow=""';

describe("the mod-frame fence", { timeout: 60_000 }, () => {
  it("allows ModFrame's frame in its one shape", async () => {
    const code = frame(
      `key={s} ref={null} ${SEALED} title={s} className={s} style={{ height: 3 }} onLoad={() => {}}`,
    );
    expect(await refusals(code, MOD_FRAME)).toEqual([]);
  });

  it("lints the real ModFrame.tsx clean", async () => {
    const results = await eslint.lintFiles([MOD_FRAME]);
    expect(results[0].messages.filter((m) => m.ruleId === "no-restricted-syntax")).toEqual([]);
  });

  it.each([
    ["no sandbox", 'src={s} allow=""'],
    ["a wider sandbox", 'src={s} sandbox="allow-scripts allow-same-origin" allow=""'],
    ["an empty sandbox", 'src={s} sandbox="" allow=""'],
    ["a sandbox in braces", 'src={s} sandbox={"allow-scripts"} allow=""'],
    ["a computed sandbox", 'src={s} sandbox={s} allow=""'],
    ["a second sandbox", `${SEALED} sandbox={s}`],
    ["no allow", 'src={s} sandbox="allow-scripts"'],
    ["a permission", 'src={s} sandbox="allow-scripts" allow="camera"'],
    ["an allow in braces", 'src={s} sandbox="allow-scripts" allow={""}'],
    ["a second allow", `${SEALED} allow={s}`],
    ["a spread", `${SEALED} {...p}`],
    ["a spread before", `{...p} ${SEALED}`],
    ["allowFullScreen", `${SEALED} allowFullScreen`],
    ["srcDoc", `${SEALED} srcDoc={s}`],
    ["a name", `${SEALED} name="x"`],
    ["credentialless", `${SEALED} credentialless`],
    ["a namespaced attribute", `${SEALED} xlink:href={s}`],
  ])("refuses ModFrame's frame with %s", async (_, attrs) => {
    expect(await refusals(frame(attrs), MOD_FRAME)).not.toEqual([]);
  });

  it.each([MOD_FRAME, ELSEWHERE])("refuses a frame made another way in %s", async (path) => {
    for (const code of [
      'document.createElement("iframe");',
      'document.createElement("IFRAME");',
      'React.createElement("iframe", { src: s });',
      'createElement("iframe");',
      'jsx("iframe", {});',
      'const Tag = "iframe";',
      "const Tag = `iframe`;",
      'document.createElement("object");',
      'document.createElement("embed");',
      'document.createElement("frame");',
      "export const O = () => <object data={s} />;",
      "export const E = () => <embed src={s} />;",
      "export const P = () => <portal src={s} />;",
      "export const R = () => <fencedframe />;",
      "export const S = () => <frameset />;",
    ]) {
      expect(await refusals(code, path), code).not.toEqual([]);
    }
  });

  it.each([MOD_FRAME, ELSEWHERE])("refuses touching a frame's sandbox in %s", async (path) => {
    for (const code of [
      'el.sandbox.add("allow-same-origin");',
      'el["sandbox"].value = s;',
      "const { sandbox } = el; void sandbox;",
      'const { ["allow"]: a } = el; void a;',
      "Object.assign(el, { sandbox: s });",
      'Object.assign(el, { "allow": s });',
      "el.allow = s;",
      "el.allowFullscreen = true;",
      'el.setAttribute("sandbox", s);',
      'el.setAttribute("SANDBOX", s);',
      'el.removeAttribute("sandbox");',
      'el.toggleAttribute("allowfullscreen");',
      'el.setAttribute("allow", s);',
      'el.setAttributeNS(null, "sandbox", s);',
      'el.removeAttributeNS(null, "allow");',
      'el.getAttributeNode("sandbox");',
    ]) {
      expect(await refusals(code, path), code).not.toEqual([]);
    }
  });

  it("refuses an iframe anywhere but ModFrame.tsx, sealed or not", async () => {
    expect(await refusals(frame(SEALED), ELSEWHERE)).not.toEqual([]);
    expect(await refusals(frame("src={s}"), ELSEWHERE)).not.toEqual([]);
    expect(await refusals(frame(SEALED), "src/components/mods/ModFrameCopy.tsx")).not.toEqual([]);
    expect(await refusals(frame(SEALED), "src/components/mods/Other.tsx")).not.toEqual([]);
  });

  it("refuses an iframe in a plain JavaScript file under src/ too", async () => {
    const [result] = await eslint.lintText(
      'export const F = () => <iframe src="pmmod://localhost/a.b/app/index.html" />;',
      { filePath: "src/components/Stray.jsx" },
    );
    expect(result.messages.filter((m) => m.fatal)).toEqual([]);
    expect(result.messages.map((m) => m.ruleId)).toContain("no-restricted-syntax");
  });

  it('lets the description tokenizer name "iframe" as data, and nowhere else', async () => {
    const code = 'export const HIDDEN = new Set(["iframe", "script"]);';
    expect(await refusals(code, "src/lib/calendarEdit/descriptionTokens.ts")).toEqual([]);
    expect(await refusals(code, "src/lib/calendarEdit/descriptionHtml.ts")).not.toEqual([]);
    expect(
      await refusals(
        'document.createElement("iframe");',
        "src/lib/calendarEdit/descriptionTokens.ts",
      ),
    ).not.toEqual([]);
  });

  it("lets tests drive frames", async () => {
    const code = 'document.createElement("iframe").sandbox.add("allow-same-origin");';
    expect(await refusals(code, "src/components/mods/ModFrame.test.tsx")).toEqual([]);
  });
});
