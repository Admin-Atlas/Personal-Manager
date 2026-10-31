// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A Google event's formatted description as PM draws it (#884, C19b): bold, italic and underlined
// text, paragraphs, lists, line breaks and links, from the codec's document (descriptionHtml.ts). The
// second sanitised renderer beside markdown.tsx, for Google event descriptions only: nothing here
// parses anything, every element is a React element named in a `switch`, and the description's
// words are React text. A link is drawn only to an address `linkTarget` takes (an absolute web or
// email address); anything else the description links to, a phone number included, shows as its
// words. Addresses written in the words are links too, as Google Calendar draws them, for drawing
// only (`textLinks`).
//
// Web links carry `target="_blank"`, so the app-wide `useExternalLinks` opens them in the browser.
// Mail links go to `onMail` (`useMailLink`), which asks the mail app for a new message to the address
// alone; `useExternalLinks` stays web-only. Its callers feed it a description from an IPC reply (the
// mirror's copy, or Google's as the editor last read it), never text the user is typing.

import { Fragment, type ReactNode } from "react";
import {
  MAX_DEPTH,
  linkTarget,
  textLinks,
  type DescDoc,
  type DescNode,
} from "../../../lib/calendarEdit/descriptionHtml";
import { stopLinkAuxClick } from "./useMailLink";

interface Props {
  doc: DescDoc;
  /** Opens a mail link's address in the mail app: `useMailLink`'s `open`. */
  onMail: (address: string) => void;
}

/** A Google event's description with the formatting PM draws: React elements from the codec's
 *  document, and a link only to a web or email address. */
export function RichDescription({ doc, onMail }: Props) {
  return (
    <div className="pm-rich" onAuxClick={stopLinkAuxClick}>
      {drawAll(doc, 0, false, onMail)}
    </div>
  );
}

function drawAll(doc: DescDoc, depth: number, inLink: boolean, onMail: Props["onMail"]) {
  // The codec's documents are never deeper than MAX_DEPTH; past it, the words alone.
  if (depth >= MAX_DEPTH) return [wordsOf(doc)];
  return doc.map((node, key) => draw(node, key, depth + 1, inLink, onMail));
}

function draw(
  node: DescNode,
  key: number,
  depth: number,
  inLink: boolean,
  onMail: Props["onMail"],
): ReactNode {
  switch (node.kind) {
    case "text":
      // Words inside a link stay its words.
      return <Fragment key={key}>{inLink ? node.text : linked(node.text, onMail)}</Fragment>;
    case "br":
      return <br key={key} />;
    case "b":
      return <b key={key}>{drawAll(node.children, depth, inLink, onMail)}</b>;
    case "i":
      return <i key={key}>{drawAll(node.children, depth, inLink, onMail)}</i>;
    case "u":
      return <u key={key}>{drawAll(node.children, depth, inLink, onMail)}</u>;
    case "p":
      return <p key={key}>{drawAll(node.children, depth, inLink, onMail)}</p>;
    case "ul":
      return <ul key={key}>{drawAll(node.children, depth, inLink, onMail)}</ul>;
    case "ol":
      return <ol key={key}>{drawAll(node.children, depth, inLink, onMail)}</ol>;
    case "li":
      return <li key={key}>{drawAll(node.children, depth, inLink, onMail)}</li>;
    case "a": {
      const words = drawAll(node.children, depth, true, onMail);
      // Again here: the model may hold a phone link (`keptLink`), which PM never opens.
      const target = linkTarget(node.href);
      if (target === null) return <Fragment key={key}>{words}</Fragment>;
      return (
        <Link key={key} target={target} onMail={onMail}>
          {words}
        </Link>
      );
    }
  }
}

/** Text with the addresses written in it drawn as links. */
function linked(text: string, onMail: Props["onMail"]): ReactNode[] {
  return textLinks(text).map((piece, key) =>
    piece.href === null ? (
      <Fragment key={key}>{piece.text}</Fragment>
    ) : (
      <Link key={key} target={piece.href} onMail={onMail}>
        {piece.text}
      </Link>
    ),
  );
}

/** A link to an address `linkTarget` took: a web one opens in the browser, a mail one in the mail
 *  app. */
function Link({
  target,
  onMail,
  children,
}: {
  target: string;
  onMail: Props["onMail"];
  children: ReactNode;
}) {
  if (/^mailto:/i.test(target)) {
    return (
      <a
        href={target}
        onClick={(e) => {
          e.preventDefault();
          onMail(target);
        }}
      >
        {children}
      </a>
    );
  }
  return (
    <a href={target} target="_blank" rel="noreferrer">
      {children}
    </a>
  );
}

/** A document's words with nothing around them, read without recursion. */
function wordsOf(doc: DescDoc): string {
  let words = "";
  const stack: DescNode[] = [...doc].reverse();
  for (let node = stack.pop(); node !== undefined; node = stack.pop()) {
    if (node.kind === "text") words += node.text;
    else if (node.kind === "br") words += " ";
    else for (let at = node.children.length - 1; at >= 0; at--) stack.push(node.children[at]);
  }
  return words;
}
