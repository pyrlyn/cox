// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// AppStore's session registry (T51.11): windows on one session share its stores, one window
// closing keeps them for the others, and the last one lets them go.

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
