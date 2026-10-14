// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// CanvasStore (T33.47.8.2, plugin-canvas §3, §5, §6): frames follow the plugin's order and an
// unchanged one keeps its value, another plugin's draw is ignored, a stop keeps the last frames,
// hiding forgets them, and a click's attachment is one chip per canvas.

import CoxClient
import Foundation
import Testing

@testable import CoxModel

private let design = PluginCanvas(
  plugin: "weft", title: "Design", symbol: "rectangle.on.rectangle", selectAttr: "data-weft-path")
private let notes = PluginCanvas(
  plugin: "notes", title: "Notes", symbol: "note.text", selectAttr: "data-note")

private func frame(_ id: String, _ html: String = "<p>v1</p>") -> CanvasFrame {
  CanvasFrame(id: id, title: id.capitalized, widthPt: 390, html: html)
}

private let homeAttach = CanvasAttach(name: "Home screen", text: "screen#home")
private let cartAttach = CanvasAttach(name: "Cart screen", text: "screen#cart")

/// A shown canvas, the composer it attaches to and the fixture behind both.
@MainActor
private struct Pane {
  let store: CanvasStore
  let composer: ComposerStore
  let session: FixtureSession

  /// Shows the first canvas and folds the draw its show sent.
  static func shown() async throws -> Pane {
    let session = FixtureSession(fixture: Fixture(batches: [], snapshot: []))
    session.plugins.canvases.declare(
      [design, notes],
      drawn: [
        CanvasUpdate(
          plugin: "weft", order: ["home", "cart"], frames: [frame("home"), frame("cart")])
      ],
      answers: ["screen#home": homeAttach, "screen#cart": cartAttach])
    let composer = ComposerStore(session: SessionStore(session: session))
    let store = try #require(CanvasStore(composer: composer))
    await store.load()
    await store.show(widthPt: 800, heightPt: 600)
    if let drawn = await session.nextCanvas() { store.apply(drawn) }
    return Pane(store: store, composer: composer, session: session)
  }
}

@MainActor
@Suite struct CanvasStoreTests {
  @Test func theFirstDeclaredCanvasShowsWithItsFrames() async throws {
    let pane = try await Pane.shown()
    let store = pane.store
    let session = pane.session
    #expect(store.canvases == [design, notes])
    #expect(store.canvas == design)
    #expect(store.frames.map(\.id) == ["home", "cart"])
    #expect(session.plugins.canvases.shown == ["weft"])
  }

  @Test func framesFollowTheOrderAndAnUnchangedOneKeepsItsValue() async throws {
    let pane = try await Pane.shown()
    let store = pane.store
    let home = store.frames[0]
    store.apply(
      CanvasUpdate(plugin: "weft", order: ["new", "home"], frames: [frame("new", "<p>new</p>")]))
    #expect(store.frames.map(\.id) == ["new", "home"])
    #expect(store.frames[1] == home)
    store.apply(CanvasUpdate(plugin: "weft", order: ["home"], frames: [frame("home", "<p>v2</p>")]))
    #expect(store.frames.map(\.html) == ["<p>v2</p>"])
  }

  @Test func anotherPluginsDrawIsIgnoredAndAStopKeepsTheLastFrames() async throws {
    let pane = try await Pane.shown()
    let store = pane.store
    store.apply(CanvasUpdate(plugin: "notes", order: ["n"], frames: [frame("n")]))
    #expect(store.frames.map(\.id) == ["home", "cart"])
    store.apply(CanvasUpdate(plugin: "weft", order: [], frames: [], stopped: true))
    #expect(store.stopped)
    #expect(store.frames.map(\.id) == ["home", "cart"])
  }

  @Test func showingAnotherCanvasHidesTheFirstAndHidingForgetsTheFrames() async throws {
    let pane = try await Pane.shown()
    let store = pane.store
    let session = pane.session
    await store.show("notes", widthPt: 800, heightPt: 600)
    #expect(session.plugins.canvases.hidden == ["weft"])
    #expect(store.canvas == notes && store.frames.isEmpty)
    await store.hide()
    #expect(store.shown == nil)
    #expect(session.plugins.canvases.hidden == ["weft", "notes"])
  }

  @Test func onlyAChangedSizeIsSentAgain() async throws {
    let pane = try await Pane.shown()
    let store = pane.store
    let session = pane.session
    await store.resize(widthPt: 800, heightPt: 600)
    await store.resize(widthPt: 900, heightPt: 600)
    #expect(session.plugins.canvases.shown == ["weft", "weft"])
  }

  @Test func aSelectionAddsOneChipAndASecondReplacesIt() async throws {
    let pane = try await Pane.shown()
    let store = pane.store
    let composer = pane.composer
    let session = pane.session
    composer.attach(Data("x".utf8), name: "x.txt", type: nil)
    await store.select(frame: "home", value: "screen#home")
    await store.select(frame: "cart", value: "screen#cart")
    #expect(composer.attachments.map(\.name) == ["x.txt", "Cart screen"])
    #expect(composer.attachments[1].mediaType == "text/plain")
    #expect(composer.attachments[1].dataB64 == Data("screen#cart".utf8).base64EncodedString())
    // No attachment for this value, and a frame not on the canvas is not asked about.
    await store.select(frame: "home", value: "nothing")
    await store.select(frame: "gone", value: "screen#home")
    #expect(composer.attachments.map(\.name) == ["x.txt", "Cart screen"])
    #expect(session.plugins.canvases.selected.count == 3)
  }

  @Test func aRemovedChipIsNotBroughtBack() async throws {
    let pane = try await Pane.shown()
    let store = pane.store
    let composer = pane.composer
    await store.select(frame: "home", value: "screen#home")
    composer.removeAttachment(at: 0)
    await store.select(frame: "cart", value: "screen#cart")
    #expect(composer.attachments.map(\.name) == ["Cart screen"])
  }

  @Test func theDrawsArriveThroughThePump() async throws {
    let pane = try await Pane.shown()
    let store = pane.store
    let session = pane.session
    let pump = Task { await store.pump() }
    session.plugins.canvases.queue(
      CanvasUpdate(plugin: "weft", order: ["cart"], frames: []))
    session.plugins.canvases.close()
    await pump.value
    #expect(store.frames.map(\.id) == ["cart"])
  }
}
