// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// AppStore's session registry (T51.11): windows on one session share its stores, one window
// closing keeps them for the others, and the last one lets them go. One canvas store per session,
// fed by the session's own pull (T33.47.8.7).

import CoxClient
import Foundation
import Testing

@testable import CoxModel

@MainActor
private func emptySession() -> FixtureSession {
  FixtureSession(fixture: Fixture(batches: [], snapshot: []))
}

@MainActor
@Test func twoWindowsOnOneSessionShareOneStore() {
  let app = AppStore()
  let (main, popped) = (UUID(), UUID())
  let first = app.adopt(emptySession(), window: main)
  let joined = app.join("fixture", window: popped)
  #expect(joined?.store === first.store)
  #expect(joined?.composer === first.composer)
}

@MainActor
@Test func aSecondOpenOfTheSameSessionJoinsTheFirst() {
  let app = AppStore()
  let first = app.adopt(emptySession(), window: UUID())
  let second = app.adopt(emptySession(), window: UUID())
  #expect(second.store === first.store)
}

@MainActor
@Test func closingOneWindowKeepsTheSessionForTheOther() {
  let app = AppStore()
  let (main, popped) = (UUID(), UUID())
  let first = app.adopt(emptySession(), window: main)
  _ = app.join("fixture", window: popped)
  app.release("fixture", window: main)
  #expect(app.isOpen("fixture"))
  #expect(app.join("fixture", window: UUID())?.store === first.store)
}

@MainActor
@Test func theLastWindowLetsTheSessionGo() {
  let app = AppStore()
  let window = UUID()
  _ = app.adopt(emptySession(), window: window)
  app.release("fixture", window: window)
  #expect(!app.isOpen("fixture"))
  #expect(app.join("fixture", window: UUID()) == nil)
}

@MainActor
@Test func twoWindowsShareOneCanvasAndASwitchMakesAFreshOne() throws {
  let app = AppStore()
  let first = app.adopt(emptySession(), window: UUID())
  let canvas = try #require(first.canvas)
  #expect(app.join("fixture", window: UUID())?.canvas === canvas)
  let switched = try #require(app.replace("fixture", with: emptySession()))
  #expect(switched.canvas != nil && switched.canvas !== canvas)
}

@MainActor
@Test func theSessionsPullFoldsTheCanvasDraws() async throws {
  let session = emptySession()
  session.plugins.canvases.declare(
    [PluginCanvas(plugin: "weft", title: "Design", symbol: "rectangle", selectAttr: "data-x")],
    drawn: [
      CanvasUpdate(
        plugin: "weft", order: ["home"],
        frames: [CanvasFrame(id: "home", title: "Home", widthPt: 390, html: "<p>home</p>")])
    ])
  let (app, window) = (AppStore(), UUID())
  let canvas = try #require(app.adopt(session, window: window).canvas)
  await canvas.load()
  await canvas.show(widthPt: 800, heightPt: 600)
  // The draw arrives on the pull's own task, so give it turns until it lands.
  var turns = 0
  while canvas.frames.isEmpty, turns < 1000 {
    await Task.yield()
    turns += 1
  }
  #expect(canvas.frames.map(\.id) == ["home"])
  // Closing the session ends the pull, which would otherwise wait for ever.
  app.release("fixture", window: window)
  #expect(session.isClosed)
}
