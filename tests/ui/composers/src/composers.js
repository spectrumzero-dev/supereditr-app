// The composer matrix's fields: real editors and simulations of others, each holding the same text.
// The UI smoke tests (tests/ui/smoke.swift) review and fix every field the same way.
//
// Each field is a section with the editable element (its DOM id is the field's id), an <output>
// (`<id>-state`) that describes what the editor's DOM now holds, and a button (`reset-<id>`) that
// puts the text back. The smoke test reads the output and presses the button through Accessibility.

import { Editor } from "@tiptap/core";
import { DOMParser as ProseMirrorParser } from "@tiptap/pm/model";
import StarterKit from "@tiptap/starter-kit";
import { $getRoot, createEditor } from "lexical";
import { registerRichText, HeadingNode, QuoteNode } from "@lexical/rich-text";
import { LinkNode } from "@lexical/link";
import { ListNode, ListItemNode } from "@lexical/list";
import { CodeNode } from "@lexical/code";
import { $generateNodesFromDOM } from "@lexical/html";

// The text, by paragraph: two typos the rule-based checker fixes ("Their going", "realy"), code at
// both ends of the first paragraph (one holds a typo of its own the review must leave alone), bold, a
// link, and an empty paragraph.
const DOC = [
  [
    { code: "teh fix" },
    " first. Their going to love this ",
    { bold: "today" },
    ". Read ",
    { link: "the notes", href: "https://example.com/notes" },
    " and realy run ",
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

// The formats document: every common structure a composer holds, with typos in prose the
// rule-based checker fixes ("teh", "Secend", "realy") and the same typo in code it must leave alone.
// An empty paragraph sits before the list. A field must be read with one line per block or list
// item, fixed, and keep every structure; the smoke test compares the page's description before and
// after.
const FORMATS = [
  "<h3>Weekly notes</h3>",
  `<p>We fixed teh <strong>bold</strong> and <em>italic</em> text, the <a href="https://example.com/notes">notes</a> and <code>make teh</code> too.</p>`,
  "<p></p>",
  "<ul><li><p>Apples</p></li><li><p>Pears</p><ul><li><p>Ripe ones</p></li></ul></li><li><p>Plums</p></li></ul>",
  "<ol><li><p>First step</p></li><li><p>Secend step</p></li></ol>",
  "<blockquote><p>Quoted line with realy good news.</p></blockquote>",
  "<pre><code>let teh = 1;</code></pre>",
  "<p>Line one<br>Line two after a break.</p>",
  "<p>See you soon.</p>",
].join("");

/** The formats document's structure: how many of each kind of block and mark, and the text of the
 * code, so a fix that loses or adds one shows. */
function structure(root) {
  const count = (selector) => root.querySelectorAll(selector).length;
  const code = [...root.querySelectorAll("code")].map((e) => e.textContent.replace(/[\u200B\u2060\uFEFF]/g, "")).join("|");
  return `headings=${count("h1,h2,h3,h4")} items=${count("ul > li")} nested=${count("li ul > li, li ol > li")} numbered=${count("ol > li")} quotes=${count("blockquote")} empty=${[...root.children].filter((e) => e.tagName === "P" && !e.textContent.trim()).length} blocks=${count("pre")} breaks=${count("br:not(.ProseMirror-trailingBreak)")} bold=${count("strong,b")} italic=${count("em,i")} links=${count("a[href]")} code=${code}`;
}

/** What the editor's DOM holds: how many times the typo appears (so a reader can tell the fix has
 * landed), code, bold and link text, and how many paragraphs, empty or not. */
function describe(root) {
  const clean = (s) => s.replace(/[​⁠﻿]/g, "");
  const texts = (selector) => [...root.querySelectorAll(selector)].map((e) => clean(e.textContent)).join("|");
  const blocks = [...root.children].filter((e) => /^(P|DIV|H\d|LI|BLOCKQUOTE|PRE)$/.test(e.tagName));
  const empty = blocks.filter((b) => clean(b.textContent).trim() === "").length;
  const typos = clean(root.textContent).split("Their going to love").length - 1;
  return `typo=${typos}; code=${texts("code")}; bold=${texts("strong, b")}; links=${texts("a[href]")}; paragraphs=${blocks.length}; empty=${empty}`;
}

const fields = [];

/** Adds a section for a field. `mount(host)` builds the editor and returns its editable element and
 * a function that resets it. */
function field(id, label, mount, content = null) {
  const section = document.createElement("section");
  section.innerHTML = `<h2>${label}</h2><div class="host"></div><output id="${id}-state"></output> <button id="reset-${id}">Reset</button>`;
  document.getElementById("composers").append(section);
  const { root, reset } = mount(section.querySelector(".host"), content);
  const output = section.querySelector("output");
  section.querySelector("button").addEventListener("click", () => reset());
  fields.push(() => {
    root.id = id;
    const state = content ? `${structure(root)}; text=${root.textContent.replace(/[\u200B\u2060\uFEFF]/g, "")}` : describe(root);
    if (output.textContent !== state) output.textContent = state;
  });
}

// A plain editable div: Chrome's own paste, which keeps the HTML's formatting.
function editableDiv(host, content) {
  const root = document.createElement("div");
  root.contentEditable = "true";
  root.className = "editor";
  // An editable div shows an empty paragraph only with a line break in it, as editors give it.
  const reset = () => (root.innerHTML = content?.replace("<p></p>", "<p><br></p>") ?? html({ br: true }));
  reset();
  host.append(root);
  return { root, reset };
}
field("code", "Editable div (pastes HTML)", editableDiv);

// Like Claude's desktop composer, a ProseMirror editor that pastes from the plain text only, reads
// it as Markdown, and pads inline code with a zero-width space on each side. Each line of the
// pasted text becomes a paragraph, an empty line an empty one; as in ProseMirror's own paste, a
// single line goes into the paragraph at the selection rather than starting a new one.
function markdownComposer(host, content) {
  const markdownLine = (line) =>
    escape(line).replace(/(`+)(.+?)\1(?!`)/g, (_, fence, code) => {
      const inner = code.length > 1 && code.startsWith(" ") && code.endsWith(" ") ? code.slice(1, -1) : code;
      return `${ZWSP}<code>${inner}</code>${ZWSP}`;
    });
  const editor = new Editor({
    element: host,
    extensions: [StarterKit],
    content: content ?? html({ pad: true }),
    editorProps: {
      attributes: { class: "editor" },
      handlePaste: (view, event) => {
        const text = event.clipboardData?.getData("text/plain") ?? "";
        const dom = document.createElement("div");
        dom.innerHTML = text.split(/\r?\n/).map((line) => `<p>${line ? markdownLine(line) : ""}</p>`).join("");
        const slice = ProseMirrorParser.fromSchema(view.state.schema).parseSlice(dom, { preserveWhitespace: "full" });
        view.dispatch(view.state.tr.replaceSelection(slice).scrollIntoView());
        return true;
      },
    },
  });
  return { root: editor.view.dom, reset: () => editor.commands.setContent(content ?? html({ pad: true })) };
}
field("markdown", "Markdown composer (plain-text paste)", markdownComposer);

// TipTap on ProseMirror, as in many chat and issue-tracker composers.
function tiptap(host, content) {
  const editor = new Editor({ element: host, extensions: [StarterKit], content: content ?? html(), editorProps: { attributes: { class: "editor" } } });
  return { root: editor.view.dom, reset: () => editor.commands.setContent(content ?? html()) };
}
field("tiptap", "TipTap (ProseMirror)", tiptap);

// Lexical, with rich text and links.
function lexical(host, content) {
  const root = document.createElement("div");
  root.contentEditable = "true";
  root.className = "editor";
  host.append(root);
  const editor = createEditor({ namespace: "smoke", nodes: [HeadingNode, QuoteNode, LinkNode, ListNode, ListItemNode, CodeNode], onError: (e) => console.error(e) });
  editor.setRootElement(root);
  registerRichText(editor);
  const reset = () =>
    editor.update(() => {
      const nodes = $generateNodesFromDOM(editor, new DOMParser().parseFromString(content ?? html(), "text/html"));
      $getRoot().clear().append(...nodes);
    });
  reset();
  return { root, reset };
}
field("lexical", "Lexical", lexical);

// The formats document in each editor.
field("formats-div", "Formats: editable div", editableDiv, FORMATS);
field("formats-markdown", "Formats: Markdown composer", markdownComposer, FORMATS);
field("formats-tiptap", "Formats: TipTap", tiptap, FORMATS);
field("formats-lexical", "Formats: Lexical", lexical, FORMATS);

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
