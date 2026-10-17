// The pointer rule through the real Rust core (T37.51, A141): each fact the rule weighs wins in
// the order `cox_render::pointer` fixes, so a client that only converts cannot drift from it.

import Testing

@testable import CoxCore

@Suite struct PointerBridgeTests {
  private func answer(
    clickable: Bool = false, selectable: Bool = false, pressed: Bool = false,
    blocked: Bool = false
  ) -> CorePointer {
    corePointer(
      clickable: clickable, selectable: selectable, pressed: pressed, blocked: blocked)
  }

  @Test func blockedWinsOverEverything() {
    #expect(
      answer(clickable: true, selectable: true, pressed: true, blocked: true) == .notAllowed)
  }

  @Test func pressedWinsOverClickable() {
    #expect(answer(clickable: true, pressed: true) == .default)
  }

  @Test func clickableWinsOverSelectable() {
    #expect(answer(clickable: true, selectable: true) == .action)
  }

  @Test func selectableAloneGivesText() {
    #expect(answer(selectable: true) == .text)
  }

  @Test func nothingSetGivesTheDefault() {
    #expect(answer() == .default)
  }
}
