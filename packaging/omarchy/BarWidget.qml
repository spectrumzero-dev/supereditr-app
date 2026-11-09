import QtQuick
import QtQuick.Effects
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

// SuperEditr in Omarchy's bar: whether it's running, its shortcuts, and its actions. Every action
// runs the supereditr command, which hands its argument to the running app and exits.
BarWidget {
  id: root
  moduleName: "app.supereditr"

  property bool opened: false
  property string appState: "stopped"
  property string statusText: ""
  property string settingsText: ""
  property string pendingAction: ""

  readonly property string configDir: (Quickshell.env("XDG_CONFIG_HOME") || Quickshell.env("HOME") + "/.config") + "/app.supereditr"
  readonly property bool running: appState === "running"
  readonly property var shortcuts: Model.shortcuts(settingsText)
  readonly property color popupForeground: bar ? bar.foreground : Color.popups.text

  function open() { opened = true; refresh() }
  function close() { opened = false }
  function closeForPopoutSwitch() { close() }
  function toggle() { opened ? close() : open() }

  function refresh() {
    if (!probe.running) probe.running = true
  }

  // Review and Quick Fix read the selection in the focused window, so the popup closes first and
  // gives focus back before the command runs.
  function runAfterClosing(action) {
    pendingAction = action
    close()
    actionDelay.restart()
  }

  function openSettings() {
    close()
    Quickshell.execDetached(["supereditr"])
    settle.restart()
  }

  // Started through uwsm-app, as Omarchy starts its apps, so it runs in its own scope.
  function start() {
    close()
    Quickshell.execDetached(["uwsm-app", "--", "supereditr"])
    settle.restart()
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  Process {
    id: probe
    command: ["sh", "-c", "command -v supereditr > /dev/null && echo installed; pgrep -x supereditr > /dev/null && echo running; true"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: root.appState = Model.appState(text)
    }
  }

  FileView {
    path: root.configDir + "/status.json"
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: root.statusText = text()
    onLoadFailed: root.statusText = ""
  }

  FileView {
    path: root.configDir + "/settings.json"
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: root.settingsText = text()
    onLoadFailed: root.settingsText = ""
  }

  Timer {
    interval: 10000
    repeat: true
    running: true
    triggeredOnStart: true
    onTriggered: root.refresh()
  }

  Timer {
    id: actionDelay
    interval: 250
    onTriggered: {
      if (root.pendingAction !== "") Quickshell.execDetached(["supereditr", root.pendingAction])
      root.pendingAction = ""
    }
  }

  // The app takes a moment to start, so check again soon after starting it.
  Timer {
    id: settle
    interval: 2000
    onTriggered: root.refresh()
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    active: root.opened
    dimmed: !root.running
    tooltipText: "SuperEditr · " + Model.statusLine(root.appState, Model.version(root.statusText))
    onPressed: function(mouseButton) {
      if (mouseButton === Qt.RightButton && root.appState !== "missing") root.openSettings()
      else root.toggle()
    }

    iconComponent: Component {
      Item {
        width: button.opticalSize
        height: button.opticalSize

        // The glyph is black; the bar's color is painted through its alpha so it follows the theme.
        Image {
          id: glyph
          anchors.fill: parent
          source: Qt.resolvedUrl("icons/glyph.svg")
          sourceSize.width: Math.round(width * Screen.devicePixelRatio)
          sourceSize.height: Math.round(height * Screen.devicePixelRatio)
          fillMode: Image.PreserveAspectFit
          visible: false
          layer.enabled: true
        }

        Rectangle {
          anchors.fill: glyph
          color: button.foreground
          layer.enabled: true
          layer.effect: MultiEffect {
            maskEnabled: true
            maskSource: glyph
            maskThresholdMin: 0.5
            maskSpreadAtMin: 1.0
          }
        }
      }
    }
  }

  PopupCard {
    id: popup
    anchorItem: button
    bar: root.bar
    owner: root
    open: root.opened
    contentWidth: popup.fittedContentWidth(Style.space(300))
    contentHeight: popup.fittedContentHeight(content.implicitHeight, Style.space(520))

    Column {
      id: content
      anchors.fill: parent
      spacing: Style.spacing.md

      Row {
        width: parent.width
        spacing: Style.spacing.md

        Image {
          width: Style.space(34)
          height: width
          source: Qt.resolvedUrl("icons/app-icon.svg")
          sourceSize.width: width * 2
          sourceSize.height: height * 2
          fillMode: Image.PreserveAspectFit
          smooth: true
        }

        Column {
          width: parent.width - Style.space(34) - parent.spacing
          anchors.verticalCenter: parent.verticalCenter
          spacing: Style.spacing.xxs

          Text {
            width: parent.width
            text: "SuperEditr"
            textFormat: Text.PlainText
            color: root.popupForeground
            font.family: Style.font.family
            font.pixelSize: Style.font.title
            font.bold: true
          }

          Text {
            width: parent.width
            text: Model.statusLine(root.appState, Model.version(root.statusText))
            textFormat: Text.PlainText
            color: root.popupForeground
            opacity: 0.62
            font.family: Style.font.family
            font.pixelSize: Style.font.caption
          }
        }
      }

      ActionButton {
        visible: root.running
        text: "Review selection"
        iconText: Model.glyph("review")
        shortcut: root.shortcuts.review
        onClicked: root.runAfterClosing("review")
      }

      ActionButton {
        visible: root.running
        text: "Quick fix selection"
        iconText: Model.glyph("quickFix")
        shortcut: root.shortcuts.quickFix
        onClicked: root.runAfterClosing("quick-fix")
      }

      ActionButton {
        visible: root.appState === "stopped"
        text: "Start SuperEditr"
        iconText: Model.glyph("start")
        onClicked: root.start()
      }

      Text {
        visible: root.appState === "missing"
        width: parent.width
        text: "Install SuperEditr to proofread selected text in any app with its shortcut."
        textFormat: Text.PlainText
        color: root.popupForeground
        opacity: 0.7
        font.family: Style.font.family
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }

      ActionButton {
        visible: root.appState === "missing"
        text: "How to install"
        iconText: Model.glyph("install")
        onClicked: {
          root.close()
          Qt.openUrlExternally(Model.INSTALL_URL)
        }
      }

      Rectangle {
        visible: root.appState !== "missing"
        width: parent.width
        height: Math.max(1, Style.normalBorderWidth)
        color: root.popupForeground
        opacity: 0.16
      }

      ActionButton {
        visible: root.appState !== "missing"
        text: "Open Settings"
        iconText: Model.glyph("settings")
        onClicked: root.openSettings()
      }
    }
  }

  // A full-width button, with the action's shortcut on its right when it has one ("Off" when the
  // user cleared it).
  component ActionButton: Button {
    property var shortcut: undefined

    width: parent.width
    foreground: root.popupForeground
    bordered: true
    focusable: true
    leftAlign: true

    Text {
      visible: parent.shortcut !== undefined
      anchors.right: parent.right
      anchors.rightMargin: Style.spacing.controlPaddingX
      anchors.verticalCenter: parent.verticalCenter
      text: Model.shortcutText(parent.shortcut)
      textFormat: Text.PlainText
      color: root.popupForeground
      opacity: 0.55
      font.family: Style.font.family
      font.pixelSize: Style.font.caption
    }
  }
}
