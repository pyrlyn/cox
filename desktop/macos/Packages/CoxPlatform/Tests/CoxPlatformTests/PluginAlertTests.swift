// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A plugin's alert without posting one (T33.45.7, PL§15.4): its content carries the session and
// the buttons the plugin declared, a click on one comes back as the `plugin:<id>:<command>` to
// run, adding its category keeps the others, and an alert for the session in front is left to
// the session's own notice.

import CoxClient
import Testing
import UserNotifications

@testable import CoxPlatform

private let alert = PluginAlert(
  session: "s1", plugin: "acme", warn: true, title: "Build failed", body: "3 tests",
  actions: [
    .init(label: "Rerun", id: "plugin:acme:rerun"), .init(label: "Log", id: "plugin:acme:log"),
  ])

@Test func aPluginAlertPostsItsButtonsAndEachRunsItsAction() throws {
  let (content, posted) = NotificationActions.content(for: alert)
  #expect((content.title, content.subtitle, content.body) == ("Build failed", "acme", "3 tests"))
  #expect(content.threadIdentifier == "s1")
  let category = try #require(posted)
  #expect(content.categoryIdentifier == category.identifier)
  #expect(category.actions.map(\.identifier) == ["plugin:acme:rerun", "plugin:acme:log"])
  #expect(category.actions.map(\.title) == ["Rerun", "Log"])
  #expect(category.actions.allSatisfy { $0.options.contains(.authenticationRequired) })
  let route = NotificationActions.plugin(action: "plugin:acme:log", userInfo: content.userInfo)
  #expect(route == PluginRoute(session: "s1", id: "plugin:acme:log"))
  // The app's own actions are not a plugin's.
  #expect(
    NotificationActions.plugin(action: NotificationActions.allow, userInfo: content.userInfo) == nil
  )
}

@Test func anAlertWithoutButtonsHasNoCategory() {
  let plain = PluginAlert(session: "s1", plugin: "acme", title: "Done", body: "")
  #expect(NotificationActions.content(for: plain).category == nil)
}

@Test func addingAPluginCategoryKeepsTheApprovalOnes() throws {
  let category = try #require(NotificationActions.content(for: alert).category)
  let merged = NotificationActions.merged(NotificationActions.categories, [category])
  #expect(
    Set(merged.map(\.identifier))
      == [NotificationActions.approval, NotificationActions.question, category.identifier])
  #expect(NotificationActions.merged(merged, [category]).count == merged.count)
}

@Test func theSessionInFrontKeepsItsAlertAsANotice() {
  let front = FrontSession()
  front.show("s1")
  #expect(MacHost(secrets: NoSecrets(), front: front).pluginNotice(alert) == false)
  front.hide("s2")
  #expect(front.shows("s1"))
  front.hide("s1")
  #expect(!front.shows("s1"))
}

/// No Keychain: a test never reads the real one.
private struct NoSecrets: SecretStore {
  func secret(for section: String) throws -> String? { nil }
  func store(_ secret: String, for section: String) throws {}
  func remove(for section: String) throws {}
}
