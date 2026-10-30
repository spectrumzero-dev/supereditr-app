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
import Quill from "quill";
import quillCSS from "quill/dist/quill.core.css";
import { createElement as h, useState } from "react";
import { createRoot } from "react-dom/client";
import { createEditor as createSlate, Transforms, Text } from "slate";
import { Slate, Editable, withReact } from "slate-react";
import { withHistory } from "slate-history";

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

/** Adds a section for a field. `mount(host)` builds the editor and returns its editable element
 * (or a function that finds it, for an editor that renders later) and a function that resets it. */
function field(id, label, mount) {
  const section = document.createElement("section");
  section.innerHTML = `<h2>${label}</h2><div class="host"></div><output id="${id}-state"></output> <button id="reset-${id}">Reset</button>`;
  document.getElementById("composers").append(section);
  const { root, reset } = mount(section.querySelector(".host"));
  const editable = typeof root === "function" ? root : () => root;
  const output = section.querySelector("output");
  section.querySelector("button").addEventListener("click", () => reset());
  fields.push(() => {
    const element = editable();
    if (!element) return;
    element.id = id;
    const state = describe(element);
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

// Quill, with its own clipboard handling.
field("quill", "Quill", (host) => {
  const quill = new Quill(host, { modules: { toolbar: false } });
  quill.clipboard.dangerouslyPasteHTML(html());
  quill.root.classList.add("editor");
  return { root: quill.root, reset: () => quill.clipboard.dangerouslyPasteHTML(html()) };
});

// Slate in React, with HTML paste as in Slate's paste-html example.
field("slate", "Slate (React)", (host) => {
  const value = () =>
    DOC.map((p) => ({
      type: "paragraph",
      children: p.length
        ? p.map((part) =>
            typeof part === "string"
              ? { text: part }
              : part.code
                ? { text: part.code, code: true }
                : part.bold
                  ? { text: part.bold, bold: true }
                  : { type: "link", url: part.href, children: [{ text: part.link }] },
          )
        : [{ text: "" }],
    }));
  // HTML to Slate nodes: paragraphs, links, and code and bold marks.
  const deserialize = (node, marks = {}) => {
    if (node.nodeType === Node.TEXT_NODE) return [{ text: node.textContent, ...marks }];
    if (node.nodeType !== Node.ELEMENT_NODE) return [];
    const tag = node.nodeName;
    const childMarks = { ...marks, ...(tag === "CODE" ? { code: true } : {}), ...(/^(STRONG|B)$/.test(tag) ? { bold: true } : {}) };
    const children = [...node.childNodes].flatMap((child) => deserialize(child, childMarks));
    const inline = children.length ? children : [{ text: "" }];
    if (tag === "BR") return [{ text: "\n", ...marks }];
    if (tag === "P" || tag === "DIV") return [{ type: "paragraph", children: inline.filter((c) => c.text !== "\n") }];
    if (tag === "A") return [{ type: "link", url: node.getAttribute("href"), children: inline }];
    return children;
  };
  const withHtml = (editor) => {
    const { insertData, isInline } = editor;
    editor.isInline = (element) => element.type === "link" || isInline(element);
    editor.insertData = (data) => {
      const pasted = data.getData("text/html");
      if (!pasted) return insertData(data);
      const body = new DOMParser().parseFromString(pasted, "text/html").body;
      const nodes = deserialize(body);
      const blocks = nodes.some((n) => n.type === "paragraph") ? nodes : [{ type: "paragraph", children: nodes }];
      Transforms.insertFragment(editor, blocks.map((b) => (Text.isText(b) ? { type: "paragraph", children: [b] } : b)));
    };
    return editor;
  };
  const renderElement = ({ attributes, children, element }) =>
    element.type === "link" ? h("a", { ...attributes, href: element.url }, children) : h("p", attributes, children);
  const renderLeaf = ({ attributes, children, leaf }) => {
    let inner = children;
    if (leaf.code) inner = h("code", null, inner);
    if (leaf.bold) inner = h("strong", null, inner);
    return h("span", attributes, inner);
  };
  // A new editor for each reset, as remounting is the simple way to replace a Slate document.
  function SlateEditor() {
    const [editor] = useState(() => withHtml(withHistory(withReact(createSlate()))));
    return h(Slate, { editor, initialValue: value() }, h(Editable, { className: "editor", renderElement, renderLeaf }));
  }
  let reset;
  function SlateField() {
    const [generation, setGeneration] = useState(0);
    reset = () => setGeneration((n) => n + 1);
    return h(SlateEditor, { key: generation });
  }
  createRoot(host).render(h(SlateField));
  return { root: () => host.querySelector("[contenteditable]"), reset: () => reset() };
});

const style = document.createElement("style");
style.textContent = `${quillCSS}
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
