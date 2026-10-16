// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `coxPointer`'s check (T37.51, DS§6.1, DS§8): every shape the core can answer maps to one
// platform pointer, the modifier asks the injected rule instead of deciding, and a catalogue
// style reports a disabled control as blocked.

import AppKit
import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct PointerTests {
  /// `PointerStyle` is not `Equatable`; its description names the style.
  private func named(_ style: PointerStyle?) -> String { String(describing: style) }

  @Test func everyShapeMapsToItsPlatformPointer() {
    #expect(named(PointerShape.default.style) == named(.default))
    #expect(named(PointerShape.action.style) == named(.link))
    #expect(named(PointerShape.text.style) == named(.horizontalText))
    #expect(PointerShape.notAllowed.style == nil)
    #expect(named(PointerShape.resize(.vertical).style) == named(.rowResize))
    #expect(named(PointerShape.resize(.horizontal).style) == named(.columnResize))
  }

  @Test func shapesAreToldApart() {
    let shapes: [PointerShape] = [
      .default, .action, .text, .resize(.vertical), .resize(.horizontal),
    ]
    #expect(Set(shapes.map { named($0.style) }).count == shapes.count)
  }

  @Test func notAllowedUsesTheSystemCursor() {
    #expect(PointerShape.notAllowed.cursor === NSCursor.operationNotAllowed)
    #expect(PointerShape.action.cursor == nil)
  }

  @Test func theModifierAsksTheInjectedRule() {
    let asked = Recorder()
    let interaction = PointerInteraction(clickable: true, pressed: true)
    layOut(Color.clear.coxPointer(interaction), rule: asked.rule(answering: .action))
    #expect(asked.interactions.contains(interaction))
  }

  @Test func withoutARuleEveryElementGetsTheArrow() {
    #expect(PointerRule.arrow.shape(PointerInteraction(clickable: true)) == .default)
    #expect(PointerRule.arrow.shape(PointerInteraction(blocked: true)) == .default)
  }

  @Test func aDisabledControlIsBlockedAndAnEnabledOneClickable() {
    #expect(ControlState.disabled.interaction().blocked)
    #expect(ControlState.disabled.interaction().clickable)
    for state in [ControlState.rest, .hovered, .pressed] {
      #expect(state.interaction() == PointerInteraction(clickable: true))
    }
  }

  @Test func aChosenControlIsPressedForTheRule() {
    #expect(ControlState.rest.interaction(isChosen: true).pressed)
    #expect(!ControlState.rest.interaction().pressed)
  }

  @Test func aCatalogueButtonReportsItsStateToTheRule() {
    let asked = Recorder()
    layOut(
      Button("Go") {}.buttonStyle(CoxButtonStyle(.primary)).disabled(true),
      rule: asked.rule(answering: .notAllowed))
    #expect(asked.interactions.contains(PointerInteraction(clickable: true, blocked: true)))

    let enabled = Recorder()
    layOut(
      Button("Go") {}.buttonStyle(CoxButtonStyle(.primary)),
      rule: enabled.rule(answering: .action))
    #expect(enabled.interactions.contains(PointerInteraction(clickable: true)))
  }

  /// Hosts `view` and lays it out, which evaluates every body that reads the environment.
  private func layOut(_ view: some View, rule: PointerRule) {
    let host = NSHostingView(rootView: view.environment(\.pointerRule, rule))
    host.frame = NSRect(origin: .zero, size: host.fittingSize)
    host.layoutSubtreeIfNeeded()
    host.displayIfNeeded()
  }
}

/// What the rule was asked, kept out of the `@Sendable` closure through a lock.
private final class Recorder: @unchecked Sendable {
  private let lock = NSLock()
  private var asked: [PointerInteraction] = []

  var interactions: [PointerInteraction] { lock.withLock { asked } }

  func rule(answering shape: PointerShape) -> PointerRule {
    PointerRule { [self] interaction in
      lock.withLock { asked.append(interaction) }
      return shape
    }
  }
}
