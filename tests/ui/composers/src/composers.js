// The composer matrix's fields: real editors and simulations of others, each holding the same text.
// The UI smoke tests (tests/ui/smoke.swift) review and fix every field the same way.
//
// Each field is a section with the editable element (its DOM id is the field's id), an <output>
// (`<id>-state`) that describes what the editor's DOM now holds, and a button (`reset-<id>`) that
// puts the text back. The smoke test reads the output and presses the button through Accessibility.

import { Editor } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";
import { $getRoot, createEditor } from "lexical";
import { registerRichText, HeadingNode, QuoteNode } from "@lexical/rich-text";
import { LinkNode } from "@lexical/link";
import { $generateNodesFromDOM } from "@lexical/html";

// The text, by paragraph: a typo the rule-based checker fixes ("Their going"), code at both ends of
// the first paragraph (one holds a typo of its own the review must leave alone), bold, a link, and
// an empty paragraph.
const DOC = [
  [
    { code: "teh fix" },
    " first. Their going to love this ",
    { bold: "today" },
    ". Read ",
    { link: "the notes", href: "https://example.com/notes" },
    " and run ",
    { code: "their going to work" },
  ],
  [],
  ["See you soon."],
];

const ZWSP = "​";
const escape = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** The text as HTML, one <p> per paragraph. `pad` puts a zero-width space on each side of code;
 * `br` gives an empty paragraph a line break, which a plain editable div needs to show it but an
 * editor would read as a line break of the author's. */
function html({ pad = false, br = false } = {}) {
  const inline = (part) => {
    if (typeof part === "string") return escape(part);
    if (part.code) return pad ? `${ZWSP}<code>${escape(part.code)}</code>${ZWSP}` : `<code>${escape(part.code)}</code>`;
    if (part.bold) return `<strong>${escape(part.bold)}</strong>`;
    return `<a href="${part.href}">${escape(part.link)}</a>`;
  };
  return DOC.map((p) => `<p>${p.length ? p.map(inline).join("") : br ? "<br>" : ""}</p>`).join("");
}

/** What the editor's DOM holds: code, bold and link text, and how many paragraphs, empty or not. */
function describe(root) {
  const clean = (s) => s.replace(/[​⁠﻿]/g, "");
  const texts = (selector) => [...root.querySelectorAll(selector)].map((e) => clean(e.textContent)).join("|");
  const blocks = [...root.children].filter((e) => /^(P|DIV|H\d|LI|BLOCKQUOTE|PRE)$/.test(e.tagName));
  const empty = blocks.filter((b) => clean(b.textContent).trim() === "").length;
  return `code=${texts("code")}; bold=${texts("strong, b")}; links=${texts("a[href]")}; paragraphs=${blocks.length}; empty=${empty}`;
}

const fields = [];

/** Adds a section for a field. `mount(host)` builds the editor and returns its editable element and
 * a function that resets it. */
function field(id, label, mount) {
  const section = document.createElement("section");
  section.innerHTML = `<h2>${label}</h2><div class="host"></div><output id="${id}-state"></output> <button id="reset-${id}">Reset</button>`;
  document.getElementById("composers").append(section);
  const { root, reset } = mount(section.querySelector(".host"));
  const output = section.querySelector("output");
  section.querySelector("button").addEventListener("click", () => reset());
  fields.push(() => {
    root.id = id;
    const state = describe(root);
    if (output.textContent !== state) output.textContent = state;
  });
}

// A plain editable div: Chrome's own paste, which keeps the HTML's formatting.
field("code", "Editable div (pastes HTML)", (host) => {
  const root = document.createElement("div");
  root.contentEditable = "true";
  root.className = "editor";
  root.innerHTML = html({ br: true });
  host.append(root);
  return { root, reset: () => (root.innerHTML = html({ br: true })) };
});

// Like Claude's desktop composer, a ProseMirror editor that pastes from the plain text only, reads
// it as Markdown, and pads inline code with a zero-width space on each side. Each line of the
// pasted text becomes a paragraph, an empty line an empty one.
field("markdown", "Markdown composer (plain-text paste)", (host) => {
  const markdownLine = (line) =>
    escape(line).replace(/(`+)(.+?)\1(?!`)/g, (_, fence, code) => {
      const inner = code.length > 1 && code.startsWith(" ") && code.endsWith(" ") ? code.slice(1, -1) : code;
      return `${ZWSP}<code>${inner}</code>${ZWSP}`;
    });
  const editor = new Editor({
    element: host,
    extensions: [StarterKit],
    content: html({ pad: true }),
    editorProps: {
      attributes: { class: "editor" },
      handlePaste: (_view, event) => {
        const text = event.clipboardData?.getData("text/plain") ?? "";
        const pasted = text.split(/\r?\n/).map((line) => `<p>${line ? markdownLine(line) : ""}</p>`).join("");
        editor.commands.insertContent(pasted, { parseOptions: { preserveWhitespace: "full" } });
        return true;
      },
    },
  });
  return { root: editor.view.dom, reset: () => editor.commands.setContent(html({ pad: true })) };
});

// TipTap on ProseMirror, as in many chat and issue-tracker composers.
field("tiptap", "TipTap (ProseMirror)", (host) => {
  const editor = new Editor({ element: host, extensions: [StarterKit], content: html(), editorProps: { attributes: { class: "editor" } } });
  return { root: editor.view.dom, reset: () => editor.commands.setContent(html()) };
});

// Lexical, with rich text and links.
field("lexical", "Lexical", (host) => {
  const root = document.createElement("div");
  root.contentEditable = "true";
  root.className = "editor";
  host.append(root);
  const editor = createEditor({ namespace: "smoke", nodes: [HeadingNode, QuoteNode, LinkNode], onError: (e) => console.error(e) });
  editor.setRootElement(root);
  registerRichText(editor);
  const reset = () =>
    editor.update(() => {
      const nodes = $generateNodesFromDOM(editor, new DOMParser().parseFromString(html(), "text/html"));
      $getRoot().clear().append(...nodes);
    });
  reset();
  return { root, reset };
});

const style = document.createElement("style");
style.textContent = `
.editor { border: 1px solid #888; padding: 4px; width: 34em; min-height: 3em; }
.editor p { margin: 0 0 4px; }
section { margin: 6px 0; }
h2 { font-size: 13px; margin: 0; }
output { font: 11px monospace; }`;
document.head.append(style);

// Keep every field's description current.
const refresh = () => fields.forEach((describeField) => describeField());
new MutationObserver(() => requestAnimationFrame(refresh)).observe(document.getElementById("composers"), {
  subtree: true,
  childList: true,
  characterData: true,
});
refresh();
