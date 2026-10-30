import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"

// One MMS picture inside a conversation bubble (OL-032). The backend fetches it from the phone on the
// first request (`GET /sms/attachment`), so this polls until the file is there, then shows it inline.
// While waiting, or if the phone is unreachable, a one-line placeholder takes its place.
Item {
  id: root

  property var app
  property string device: ""
  property var partId: 0
  property string uniqueId: ""
  property string preview: ""            // base64 PNG thumbnail from the phone (shown until the full picture is here)
  readonly property bool hasPreview: preview !== ""
  readonly property string previewSource: hasPreview ? "data:image/png;base64," + preview.replace(/\s+/g, "") : ""

  property string fetchState: "pending"       // pending | ready | failed
  property string path: ""
  property string error: ""

  readonly property bool fullShown: fetchState === "ready" && img.status === Image.Ready
  implicitHeight: fullShown
    ? Math.min(Style.space(320), width * img.implicitHeight / Math.max(1, img.implicitWidth))
    : (thumb.visible ? thumb.height + (note.visible ? note.implicitHeight : 0) : note.implicitHeight)

  function poll() {
    if (!app || device === "" || uniqueId === "") return
    app.request("GET", "/sms/attachment?device=" + encodeURIComponent(device) + "&part=" + partId
                + "&uid=" + encodeURIComponent(uniqueId), function(xhr) {
      if (xhr.status !== 200) { fetchState = "failed"; error = "backend offline"; retry.interval = 15000; retry.restart(); return }
      try {
        var r = JSON.parse(xhr.responseText)
        if (r.state === "ready") { path = r.path; fetchState = "ready"; retry.stop(); return }
        fetchState = r.state === "failed" ? "failed" : "pending"
        error = r.error || ""
      } catch (e) { fetchState = "failed"; error = "bad answer" }
      retry.interval = fetchState === "pending" ? 1500 : 30000
      retry.restart()
    })
  }
  Component.onCompleted: { if (uniqueId === "") { fetchState = "failed"; error = "the phone's sync has not refreshed this message yet" } else poll() }
  Timer { id: retry; repeat: false; onTriggered: root.poll() }

  Image {
    id: img
    visible: root.fetchState === "ready"
    anchors.fill: parent
    source: root.fetchState === "ready" ? "file://" + root.path : ""
    fillMode: Image.PreserveAspectFit
    horizontalAlignment: Image.AlignLeft
    asynchronous: true
    cache: true
    sourceSize.width: Math.round(root.width * 2)
  }

  // The phone's small preview stands in until (or unless) the full picture arrives.
  Image {
    id: thumb
    visible: root.hasPreview && !root.fullShown
    source: root.hasPreview ? root.previewSource : ""
    fillMode: Image.PreserveAspectFit
    smooth: true
    width: Math.min(sourceSize.width, root.width)
    height: sourceSize.width > 0 ? width * sourceSize.height / sourceSize.width : 0
  }

  UiText {
    id: note
    anchors.top: thumb.visible ? thumb.bottom : parent.top
    visible: !root.fullShown && !(root.hasPreview && root.fetchState === "pending")
    width: parent.width
    text: root.fetchState === "failed" ? "\uf03e full picture not loaded: " + root.error
        : (root.fetchState === "ready" && img.status === Image.Error) ? "\uf03e picture could not be shown"
        : "\uf03e loading picture\u2026"
    foreground: root.fetchState === "failed" ? Hues.red : Hues.muted
    font.pixelSize: Style.font.bodySmall
    wrapMode: Text.Wrap
  }
}
