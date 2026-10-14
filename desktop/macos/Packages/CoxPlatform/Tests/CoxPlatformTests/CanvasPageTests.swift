// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// CanvasPage's isolation (T33.47.7, docs/design/plugin-canvas.md §4), proved
// on the shipped macOS rather than argued: a page script does not run, an
// `https:` or `data:` image is not loaded, the page cannot navigate, and the
// host script's post arrives with the clicked element's value. A shown frame
// reports its content's height at 100% and zooms inside (T33.47.8.8), and a
// redraw keeps the scroll and the selection's outline (T33.47.8.10). Nothing here
// reaches the network: the rule list blocks it.

import Foundation
import Testing
import WebKit

@testable import CoxPlatform

/// A one-pixel PNG, so a loaded `data:` image would have a width of 1.
private let pixel =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="

private let fixture = """
  <!doctype html>
  <html><head><title>clean</title>
  <script>document.title = "ran";</script></head>
  <body>
  <section data-weft-path="screen#home/hero"><h1 id="title">Hero</h1></section>
  <img id="web" src="https://example.com/x.png" alt="web">
  <img id="data" src="data:image/png;base64,\(pixel)" alt="data">
  <a id="link" href="https://example.com/">away</a>
  </body></html>
  """

@MainActor
@Suite struct CanvasPageTests {
  private func loaded() async throws -> CanvasPage {
    let canvas = try await CanvasPage.make(selectAttribute: "data-weft-path")
    try await canvas.load(html: fixture)
    return canvas
  }

  /// Runs in the host script's world, as the app's own code does.
  private func run(_ canvas: CanvasPage, _ body: String) async throws -> Any? {
    try await canvas.page.callJavaScript(body, contentWorld: CanvasPage.world)
  }

  @Test func aPageScriptDoesNotRun() async throws {
    let canvas = try await loaded()
    #expect(try await run(canvas, "return document.title") as? String == "clean")
  }

  @Test func neitherAWebNorADataImageLoads() async throws {
    let canvas = try await loaded()
    // Give a load that was let through time to finish.
    try await Task.sleep(for: .milliseconds(300))
    for id in ["web", "data"] {
      let width = try await run(canvas, "return document.getElementById('\(id)').naturalWidth")
      #expect((width as? Int) == 0, "\(id) loaded")
    }
  }

  @Test func thePageCannotNavigate() async throws {
    let canvas = try await loaded()
    let own = canvas.page.url
    #expect(own?.scheme == CanvasPage.scheme)
    _ = try await run(canvas, "location.href = 'https://example.com/'; return true")
    canvas.page.load(URL(string: "https://example.com/"))
    canvas.page.load(URLRequest(url: own!))
    try await Task.sleep(for: .milliseconds(300))
    #expect(canvas.page.url == own)
    #expect(try await run(canvas, "return document.title") as? String == "clean")
  }

  @Test func aClickPostsTheNearestValueAndDoesNotFollowALink() async throws {
    let canvas = try await loaded()
    var values = canvas.selections.makeAsyncIterator()
    _ = try await run(canvas, "document.getElementById('title').click(); return true")
    #expect(await values.next() == "screen#home/hero")
    _ = try await run(canvas, "document.getElementById('link').click(); return true")
    try await Task.sleep(for: .milliseconds(300))
    #expect(canvas.page.url?.scheme == CanvasPage.scheme)
  }

  @Test func aNewLoadReplacesTheFrame() async throws {
    let canvas = try await loaded()
    try await canvas.load(html: "<title>second</title><p>2</p>")
    #expect(try await run(canvas, "return document.title") as? String == "second")
  }

  @Test func aShownFrameReportsItsHeightAt100PercentAndZoomsInside() async throws {
    let canvas = try await CanvasPage.make(selectAttribute: "data-weft-path")
    let tall = try await canvas.show(html: "<div style=\"height:500px\"></div>", zoom: 2)
    // The div and the body's default margins, not the zoomed 1000.
    #expect((500...540).contains(tall))
    #expect(try await run(canvas, "return document.documentElement.style.zoom") as? String == "2")
    // Shorter than the last one, in standards mode as the first was in quirks mode.
    let short = try await canvas.show(html: "<!doctype html><p>x</p>", zoom: 1)
    #expect(short > 0 && short < 100)
    try await canvas.zoom(0.5)
    #expect(try await run(canvas, "return document.documentElement.style.zoom") as? String == "0.5")
  }

  @Test func aRedrawKeepsTheScrollAndTheOutlineWhileTheValueIsThere() async throws {
    let canvas = try await CanvasPage.make(selectAttribute: "data-weft-path")
    let page = { (mark: String) in
      #"<div style="height:3000px"><p data-weft-path="screen#home" id="home">\#(mark)</p></div>"#
    }
    _ = try await canvas.show(html: page("1"), zoom: 1)
    _ = try await run(canvas, "document.getElementById('home').click(); scrollTo(0, 500); return 1")
    _ = try await canvas.show(html: page("2"), zoom: 1)
    let selected = "return document.querySelector('[data-cox-selected]')?.id ?? ''"
    #expect(try await run(canvas, selected) as? String == "home")
    #expect((try await run(canvas, "return scrollY") as? NSNumber)?.intValue == 500)
    // The value is gone from the next draw, so nothing is outlined.
    _ = try await canvas.show(html: #"<p data-weft-path="screen#cart">3</p>"#, zoom: 1)
    #expect(try await run(canvas, selected) as? String == "")
  }

  @Test func theAttributeReachesTheScriptAsJSON() {
    let script = CanvasPage.hostScript(attribute: "data-x\"];alert(1);//")
    #expect(script.contains(#"const attr = "data-x\"];alert(1);\/\/";"#))
  }
}
