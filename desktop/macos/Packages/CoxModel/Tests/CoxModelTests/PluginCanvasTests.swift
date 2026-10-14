// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The fixture's canvases (T33.47.8.1), which the store's and the pane's tests stand on: what a
// fixture declares comes back through `CanvasClient`, a show sends the drawn update, a click is
// recorded and answered, and the pull ends once the fixture is closed.

import CoxClient
import Testing

private let design = PluginCanvas(
  plugin: "weft", title: "Design", symbol: "rectangle.on.rectangle", selectAttr: "data-weft-path")
private let home = CanvasFrame(id: "home", title: "Home", widthPt: 390, html: "<p>home</p>")
private let drawn = CanvasUpdate(plugin: "weft", order: ["home"], frames: [home])

private func session() -> FixtureSession {
  let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
  session.plugins.canvases.declare(
    [design], drawn: [drawn],
    answers: ["screen#home": CanvasAttach(name: "Home screen", text: "screen#home")])
  return session
}

@Test func aFixtureListsItsCanvases() async throws {
  #expect(try await session().pluginCanvases() == [design])
}

@Test func aShowIsRecordedAndSendsTheDrawnUpdate() async throws {
  let session = session()
  try await session.showCanvas("weft", widthPt: 800, heightPt: 600)
  #expect(session.plugins.canvases.shown == ["weft"])
  #expect(await session.nextCanvas() == drawn)
}

@Test func aQueuedRedrawReachesAWaitingPull() async {
  let session = session()
  let redraw = CanvasUpdate(plugin: "weft", order: [], frames: [], stopped: true)
  async let next = session.nextCanvas()
  session.plugins.canvases.queue(redraw)
  #expect(await next == redraw)
}

@Test func aClickIsRecordedAndAnswered() async throws {
  let session = session()
  #expect(
    try await session.selectCanvas("weft", frame: "home", value: "screen#home")
      == CanvasAttach(name: "Home screen", text: "screen#home"))
  #expect(try await session.selectCanvas("weft", frame: "home", value: "other") == nil)
  #expect(
    session.plugins.canvases.selected == [
      ["weft", "home", "screen#home"], ["weft", "home", "other"],
    ])
  try await session.hideCanvas("weft")
  #expect(session.plugins.canvases.hidden == ["weft"])
}

@Test func thePullEndsOnceClosed() async {
  let session = session()
  async let next = session.nextCanvas()
  session.plugins.canvases.close()
  #expect(await next == nil)
  #expect(await session.nextCanvas() == nil)
}
