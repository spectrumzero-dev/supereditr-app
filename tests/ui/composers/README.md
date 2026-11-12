# Composer matrix editors

The UI smoke tests (`tests/ui/smoke.swift`, section 6c) open a page in Chrome with the same text in several rich-text editors and check that SuperEditr reviews and fixes each one the same way. This folder holds those editors.

- `src/composers.js` mounts each field: an editable div that pastes HTML, a Markdown composer like Claude's desktop app (a ProseMirror editor that pastes from the plain text only, reads it as Markdown, and pads inline code with zero-width spaces), TipTap, Lexical, Slate and Quill. These cover the distinct ways a composer pastes: HTML into ProseMirror (as many chat and issue-tracker composers do), HTML into Lexical, plain text read as Markdown, plain text into Slate (one block per line, as in Discord's message box, which is built on Slate), HTML into Quill (the engine of Signal Desktop's composer), and Chrome's own paste. A second set of fields holds a document with every common structure in each editor. Each field has an `<output id="<id>-state">` describing the code, bold, links and paragraphs its DOM now holds, and a `reset-<id>` button that puts the text back.
- `composers.bundle.js` is that file bundled with the editors, committed so the smoke tests never fetch anything. `LICENSES.txt` has the licence of every package in it. A package that publishes no licence file, as Slate does, has its repository's licence in `licenses/<repository name>.md`.
- `build.sh` rebuilds both from the versions pinned in `package.json` and `package-lock.json`. Run it after changing `src/composers.js` or a version, and commit the results.

To add an editor, mount it in `src/composers.js` with the shared text, rebuild, and add its id to `composerFields` in `smoke.swift`.
