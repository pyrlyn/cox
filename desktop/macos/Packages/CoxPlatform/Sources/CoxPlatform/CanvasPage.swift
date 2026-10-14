// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// One frame of a plugin's canvas (T33.47.7, docs/design/plugin-canvas.md §4):
// a `WebPage` that shows the plugin's HTML and nothing else. The HTML is
// untrusted and not filtered as a string; WebKit is the boundary, set up
// here in one place:
//
// - no page script: `allowsContentJavaScript` is off;
// - no network: a content rule list blocks every load but the frame's own
//   document;
// - no inline resources either: the document comes from this class's
//   `cox-canvas:` scheme handler with a `Content-Security-Policy` header of
//   `default-src 'none'`, since a rule list does not stop a `data:` image;
// - no navigation: the decider allows only the document load this class
//   starts, and refuses links, forms, refreshes and frames;
// - no state and no bridge: a non-persistent store, and the only message
//   handler lives in the `cox-canvas` content world, beside the host script
//   that outlines the element under the pointer and posts a click's
//   selection value. The page shares the DOM but runs no script to forge it.
//
// The app sizes each frame's box from the content height `show` measures and
// zooms inside the page, since scaling the web view from outside would put
// clicks where the content is not drawn.
//
// Separate from `BrowserController`, whose page the agent drives on the open
// web; this one never leaves its own `cox-canvas:` document.

import Foundation
import Synchronization
import WebKit

/// Why a canvas page could not be set up or loaded.
public enum CanvasFailure: Error, Equatable {
  case ruleList(String)
  case page(String)
}

@MainActor
public final class CanvasPage {
  /// The frame the pane shows.
  public let page: WebPage
  /// Each click's selection value, in order.
  public let selections: AsyncStream<String>
  /// The world the host script and its handler live in.
  static let world = WKContentWorld.world(name: "cox-canvas")
  /// The handler's name inside that world.
  static let handlerName = "coxCanvas"
  private let gate: Gate
  private let handler: Handler
  /// The plugin's selection attribute, already checked by the plugin API.
  private let attribute: String

  /// The scheme the frame's own document is served on.
  nonisolated static let scheme = "cox-canvas"
  /// Inline styles only: the page's look, and nothing it could fetch or run.
  nonisolated static let policy =
    "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"
  /// Blocks every load but a document on the frame's own scheme, which the
  /// decider then admits only when this class started it.
  static let blockEverything = """
    [{"trigger": {"url-filter": ".*"}, "action": {"type": "block"}},
     {"trigger": {"url-filter": "^cox-canvas://frame/", "resource-type": ["document"]},
      "action": {"type": "ignore-previous-rules"}}]
    """
  private static var compiled: WKContentRuleList?

  /// A page whose clicks read `selectAttribute`, already checked by the
  /// plugin API (`data-` and lowercase); it reaches the script as JSON.
  public static func make(selectAttribute: String) async throws(CanvasFailure) -> CanvasPage {
    let rules = try await ruleList()
    let (selections, sink) = AsyncStream.makeStream(of: String.self)
    let handler = Handler(sink: sink)
    let gate = Gate()
    var configuration = WebPage.Configuration()
    if let scheme = URLScheme(scheme) {
      configuration.urlSchemeHandlers[scheme] = Server(served: gate.served)
    }
    configuration.websiteDataStore = .nonPersistent()
    configuration.defaultNavigationPreferences.allowsContentJavaScript = false
    let content = configuration.userContentController
    content.add(rules)
    content.add(handler, contentWorld: world, name: handlerName)
    content.addUserScript(
      WKUserScript(
        source: hostScript(attribute: selectAttribute), injectionTime: .atDocumentEnd,
        forMainFrameOnly: true, in: world))
    let page = WebPage(configuration: configuration, navigationDecider: Decider(gate: gate))
    return CanvasPage(
      page: page, selections: selections, gate: gate, handler: handler, attribute: selectAttribute)
  }

  private init(
    page: WebPage, selections: AsyncStream<String>, gate: Gate, handler: Handler, attribute: String
  ) {
    self.page = page
    self.selections = selections
    self.gate = gate
    self.handler = handler
    self.attribute = attribute
  }

  /// Shows `html`, replacing what the frame showed; returns once loaded.
  public func load(html: String) async throws(CanvasFailure) {
    let url = gate.arm(html: html)
    do {
      for try await event in page.load(URLRequest(url: url)) where event == .finished { return }
    } catch {
      throw .page("the frame did not load: \(error.localizedDescription)")
    }
  }

  /// Shows `html` at `zoom` and returns its content's height at 100%, in
  /// points. Measured before the zoom applies, on the fresh document `load`
  /// opened, and from the body's contents: the root's and the body's own boxes
  /// fill the frame's current height in quirks mode, so they never shrink.
  /// A redraw keeps the scroll offset, and the outline while the selected
  /// value is still on the page (plugin-canvas §6).
  public func show(html: String, zoom: Double) async throws(CanvasFailure) -> Double {
    let kept = page.url == nil ? nil : try await run(Self.keep, ["attr": attribute])
    try await load(html: html)
    let height = try await run(Self.measure)
    try await self.zoom(zoom)
    if var kept = kept as? [String: Any] {
      kept["attr"] = attribute
      _ = try await run(Self.restore, kept)
    }
    return (height as? NSNumber)?.doubleValue ?? 0
  }

  static let keep = """
    const selected = document.querySelector("[data-cox-selected]");
    return { x: window.scrollX, y: window.scrollY,
      selected: selected ? selected.getAttribute(attr) ?? "" : "" };
    """

  /// Compares values rather than building a selector from one, which the
  /// page's DOM wrote.
  static let restore = """
    if (selected) {
      const el = Array.from(document.querySelectorAll("[" + attr + "]"))
        .find(e => e.getAttribute(attr) === selected);
      if (el) el.setAttribute("data-cox-selected", "");
    }
    window.scrollTo(x, y);
    """

  static let measure = """
    const range = document.createRange();
    range.selectNodeContents(document.body);
    const margin = parseFloat(getComputedStyle(document.body).marginBottom) || 0;
    return Math.ceil(range.getBoundingClientRect().bottom + window.scrollY + margin);
    """

  /// Scales the content inside the frame, as CSS `zoom` does.
  public func zoom(_ zoom: Double) async throws(CanvasFailure) {
    _ = try await run(
      "document.documentElement.style.setProperty('zoom', String(zoom))", ["zoom": zoom])
  }

  /// Runs `body` in the host script's world: the page's own script is off,
  /// and it shares only the DOM with this.
  private func run(
    _ body: String, _ arguments: [String: Any] = [:]
  ) async throws(CanvasFailure) -> Any? {
    do {
      return try await page.callJavaScript(body, arguments: arguments, contentWorld: Self.world)
    } catch {
      throw .page("the frame's script failed: \(error.localizedDescription)")
    }
  }

  private static func ruleList() async throws(CanvasFailure) -> WKContentRuleList {
    if let compiled { return compiled }
    guard let store = WKContentRuleListStore.default() else {
      throw .ruleList("no content rule list store")
    }
    do {
      let list = try await store.compileContentRuleList(
        forIdentifier: "cox-canvas-block-everything", encodedContentRuleList: blockEverything)
      guard let list else { throw CanvasFailure.ruleList("the rule list did not compile") }
      compiled = list
      return list
    } catch {
      throw .ruleList(error.localizedDescription)
    }
  }

  /// Outlines the hovered element and the selected one, cancels every
  /// click's default action, and posts the nearest selection value.
  static func hostScript(attribute: String) -> String {
    let name = (try? JSONEncoder().encode(attribute)).flatMap { String(data: $0, encoding: .utf8) }
    return """
      (() => {
        const attr = \(name ?? "\"\"");
        const style = document.createElement("style");
        style.textContent = "[data-cox-hover]{outline:2px dashed #0a84ff !important;outline-offset:2px}" +
          "[data-cox-selected]{outline:2px solid #0a84ff !important;outline-offset:2px}";
        document.documentElement.appendChild(style);
        const mark = (name, el) => {
          document.querySelectorAll("[" + name + "]").forEach(e => e.removeAttribute(name));
          if (el) el.setAttribute(name, "");
        };
        const owner = t => t instanceof Element ? t.closest("[" + attr + "]") : null;
        document.addEventListener("mouseover", e => mark("data-cox-hover", owner(e.target)), true);
        document.addEventListener("click", e => {
          e.preventDefault();
          e.stopPropagation();
          const el = owner(e.target);
          if (!el) return;
          mark("data-cox-selected", el);
          window.webkit.messageHandlers.\(handlerName).postMessage({ value: el.getAttribute(attr) });
        }, true);
      })();
      """
  }
}

/// The document the frame's own URL serves. Locked rather than main-actor
/// bound, since WebKit asks the scheme handler off the main actor.
private final class Served: Sendable {
  let current = Mutex<(url: URL, html: String)?>(nil)
}

/// Lets exactly one navigation through, to the document `CanvasPage` just
/// asked for, and hands that document to the scheme handler. A class so
/// the page and its decider share it.
@MainActor
private final class Gate {
  private var loads = 0
  /// The document the next navigation may open, until it does.
  private var armed: URL?
  let served = Served()

  func arm(html: String) -> URL {
    loads += 1
    // A fresh URL per load, so nothing cached answers it.
    let url = URL(string: "\(CanvasPage.scheme)://frame/\(loads)")!
    armed = url
    served.current.withLock { $0 = (url, html) }
    return url
  }

  func admit(_ url: URL?) -> Bool {
    guard let armed, url == armed else { return false }
    self.armed = nil
    return true
  }
}

@MainActor
private struct Decider: WebPage.NavigationDeciding {
  let gate: Gate

  func decidePolicy(
    for action: WebPage.NavigationAction, preferences: inout WebPage.NavigationPreferences
  ) async -> WKNavigationActionPolicy {
    gate.admit(action.request.url) ? .allow : .cancel
  }
}

/// Serves the frame's document with its policy header, and nothing else.
private struct Server: URLSchemeHandler {
  let served: Served

  func reply(for request: URLRequest) -> AsyncThrowingStream<URLSchemeTaskResult, any Error> {
    let served = served.current.withLock { $0 }
    return AsyncThrowingStream { output in
      guard let served, request.url == served.url,
        let response = HTTPURLResponse(
          url: served.url, statusCode: 200, httpVersion: "HTTP/1.1",
          headerFields: [
            "Content-Type": "text/html; charset=utf-8",
            "Content-Security-Policy": CanvasPage.policy,
          ])
      else {
        output.finish(throwing: URLError(.fileDoesNotExist))
        return
      }
      output.yield(.response(response))
      output.yield(.data(Data(served.html.utf8)))
      output.finish()
    }
  }
}

/// Receives the host script's posts, in the `cox-canvas` world only.
private final class Handler: NSObject, WKScriptMessageHandler {
  let sink: AsyncStream<String>.Continuation

  init(sink: AsyncStream<String>.Continuation) {
    self.sink = sink
  }

  func userContentController(
    _ controller: WKUserContentController, didReceive message: WKScriptMessage
  ) {
    // The value is the page's: Rust checks it before the plugin sees it.
    guard let body = message.body as? [String: Any], let value = body["value"] as? String else {
      return
    }
    sink.yield(value)
  }
}
