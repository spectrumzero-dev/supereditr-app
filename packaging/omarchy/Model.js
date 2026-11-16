// What the bar widget shows, worked out from SuperEditr's own files and a process check. Plain
// functions, so tests/model.test.mjs runs them under Node without Quickshell.

var INSTALL_URL = "https://supereditr.app/#omarchy"

// SuperEditr's Linux defaults, used while its settings name no shortcuts (crates/core/src/config.rs).
var DEFAULT_REVIEW = "Super+Alt+E"
var DEFAULT_QUICK_FIX = "Super+Control+Alt+Shift+E"
// The first quick-fix default, which SuperEditr moves to the new one because Omarchy 4.5 binds it.
var OLD_QUICK_FIX = "Super+Control+Alt+E"

// Nerd Font icons, which Omarchy's bar font includes.
var GLYPHS = { review: 0xF04C6, quickFix: 0xF140B, settings: 0xF0493, start: 0xF040A, install: 0xF01DA }

function glyph(name) {
  return String.fromCodePoint(GLYPHS[name])
}

// The status probe prints "installed" when the command exists and "running" when the app does.
// A running app counts even if it isn't on PATH, as when it was started from a build folder.
function appState(probeOutput) {
  var lines = String(probeOutput || "").split("\n")
  if (lines.indexOf("running") !== -1) return "running"
  if (lines.indexOf("installed") !== -1) return "stopped"
  return "missing"
}

// status.json, which SuperEditr writes at startup, says which version is running.
function version(statusText) {
  try {
    var status = JSON.parse(statusText || "")
    return status && typeof status.version === "string" ? status.version : ""
  } catch (e) {
    return ""
  }
}

function statusLine(state, runningVersion) {
  if (state === "running") return runningVersion ? "Running · version " + runningVersion : "Running"
  if (state === "stopped") return "Not running"
  return "Not installed"
}

// The shortcuts SuperEditr binds. While it runs, status.json says which it uses and which the
// desktop refused because something else has the key (null here). Otherwise they're read from
// settings.json the way SuperEditr reads them: the Linux ones live under linux_hotkeys, and a file
// without them, or one that isn't JSON, means the defaults.
function shortcuts(settingsText, statusText) {
  var live = parse(statusText)
  live = live && live.shortcuts && typeof live.shortcuts === "object" ? live.shortcuts : null
  if (live && typeof live.review === "string" && typeof live.quick_fix === "string") {
    var refused = Array.isArray(live.refused) ? live.refused : []
    return {
      review: refused.indexOf("review") === -1 ? live.review : null,
      quickFix: refused.indexOf("quick_fix") === -1 ? live.quick_fix : null
    }
  }
  var settings = parse(settingsText)
  var own = settings && typeof settings.linux_hotkeys === "object" ? settings.linux_hotkeys : null
  var quickFix = own && typeof own.quick_fix === "string" ? own.quick_fix : DEFAULT_QUICK_FIX
  return {
    review: own && typeof own.review === "string" ? own.review : DEFAULT_REVIEW,
    quickFix: quickFix === OLD_QUICK_FIX ? DEFAULT_QUICK_FIX : quickFix
  }
}

function parse(text) {
  try {
    return JSON.parse(text || "")
  } catch (e) {
    return null
  }
}

// An empty shortcut is one the user turned off; null is one the desktop refused.
function shortcutText(accelerator) {
  if (accelerator === null) return "Not bound"
  var text = String(accelerator || "").trim()
  return text === "" ? "Off" : text
}
