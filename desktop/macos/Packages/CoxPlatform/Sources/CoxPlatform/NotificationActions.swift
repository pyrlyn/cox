// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Actionable notifications (DT§5.6, T37.27, T37.44.15): an approval posts with Allow once, Deny and
// Open, a question with a typed answer, and an action on either comes back as the session and the
// `Intent` to send it, or as the session to bring forward. A plugin's alert (PL§15.4) posts with
// the buttons it declared, and a click comes back as the `plugin:<id>:<command>` to run. Separate
// from `MacHost` so the note-to-content and action-to-intent mappings are plain functions a test
// checks without posting a notification; `NotificationResponder` is the thin delegate the app
// installs, and the only part that needs the notification centre.

import CoxClient
import Foundation
import UserNotifications

/// Where a notification action goes: the session and the intent to send it.
public struct NotificationRoute: Equatable, Sendable {
  public var session: String
  public var intent: Intent

  public init(session: String, intent: Intent) {
    self.session = session
    self.intent = intent
  }
}

/// A plugin button's click: the session and the `plugin:<id>:<command>` it runs.
public struct PluginRoute: Equatable, Sendable {
  public var session: String
  public var id: String

  public init(session: String, id: String) {
    (self.session, self.id) = (session, id)
  }
}

public enum NotificationActions {
  static let approval = "cox.approval"
  static let question = "cox.question"
  static let allow = "cox.allow"
  static let deny = "cox.deny"
  static let open = "cox.open"
  static let answer = "cox.answer"
  static let sessionKey = "session"
  static let callKey = "call"
  static let pluginPrefix = "plugin:"
  static let pluginCategory = "cox.plugin."

  /// Allow once, Deny and Open on an approval (mockup 23) — allow for session and edit need the
  /// app, by design (DT§5.6) — and a typed answer on a question. Allow once asks for the unlocked
  /// Mac: it runs a command. Open brings cox forward on the session.
  public static var categories: Set<UNNotificationCategory> {
    let allow = UNNotificationAction(
      identifier: Self.allow, title: "Allow once", options: [.authenticationRequired])
    let deny = UNNotificationAction(identifier: Self.deny, title: "Deny", options: [])
    let open = UNNotificationAction(identifier: Self.open, title: "Open", options: [.foreground])
    let answer = UNTextInputNotificationAction(
      identifier: Self.answer, title: "Answer", options: [.authenticationRequired],
      textInputButtonTitle: "Send", textInputPlaceholder: "Your answer")
    return [
      UNNotificationCategory(
        identifier: approval, actions: [allow, deny, open], intentIdentifiers: [], options: []),
      UNNotificationCategory(
        identifier: question, actions: [answer], intentIdentifiers: [], options: []),
    ]
  }

  /// What `note` posts: grouped by session; an approval or question that can still be answered
  /// carries its category and the ids an action needs.
  static func content(for note: HostNote) -> UNMutableNotificationContent {
    let content = UNMutableNotificationContent()
    content.title = title(note.kind)
    content.body = note.text
    content.threadIdentifier = note.session
    content.userInfo[sessionKey] = note.session
    if let call = note.call {
      content.userInfo[callKey] = call
      switch note.kind {
      case .approval: content.categoryIdentifier = approval
      case .question: content.categoryIdentifier = question
      case .failed, .taskDone: break
      }
    }
    return content
  }

  /// What a plugin's alert posts: its plugin as subtitle, grouped by session, and its buttons in
  /// a category named by their ids, so alerts with the same buttons share one. A button runs a
  /// command as if typed, so it asks for the unlocked Mac, as Allow once does.
  static func content(
    for alert: PluginAlert
  ) -> (content: UNMutableNotificationContent, category: UNNotificationCategory?) {
    let content = UNMutableNotificationContent()
    (content.title, content.subtitle, content.body) = (alert.title, alert.plugin, alert.body)
    content.threadIdentifier = alert.session
    content.userInfo[sessionKey] = alert.session
    if alert.warn { content.sound = .default }
    guard !alert.actions.isEmpty else { return (content, nil) }
    content.categoryIdentifier = pluginCategory + alert.actions.map(\.id).joined(separator: " ")
    let actions = alert.actions.map {
      UNNotificationAction(identifier: $0.id, title: $0.label, options: [.authenticationRequired])
    }
    let category = UNNotificationCategory(
      identifier: content.categoryIdentifier, actions: actions, intentIdentifiers: [], options: [])
    return (content, category)
  }

  /// `adding` over `existing`: the centre keeps one set, and replacing it whole would strip the
  /// buttons off a plugin alert still on screen.
  static func merged(
    _ existing: Set<UNNotificationCategory>, _ adding: Set<UNNotificationCategory>
  ) -> Set<UNNotificationCategory> {
    let replaced = Set(adding.map(\.identifier))
    return existing.filter { !replaced.contains($0.identifier) }.union(adding)
  }

  /// The plugin action a button runs; `nil` for any other action.
  public static func plugin(action: String, userInfo: [AnyHashable: Any]) -> PluginRoute? {
    guard action.hasPrefix(pluginPrefix), let session = userInfo[sessionKey] as? String
    else { return nil }
    return PluginRoute(session: session, id: action)
  }

  /// The intent an action sends; `nil` for Open, a plain click, an unknown action or an empty
  /// answer. Allow once is the one-call approval, never a standing rule.
  public static func route(
    action: String, userInfo: [AnyHashable: Any], text: String?
  ) -> NotificationRoute? {
    guard let session = userInfo[sessionKey] as? String, let call = userInfo[callKey] as? String
    else { return nil }
    let intent: Intent
    switch action {
    case allow: intent = .approve(call: call, decision: .allow)
    case deny: intent = .approve(call: call, decision: .deniedByUser)
    case answer:
      guard let text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty
      else { return nil }
      intent = .answer(question: call, text: text)
    default: return nil
    }
    return NotificationRoute(session: session, intent: intent)
  }

  /// The session Open or a plain click on the notification brings forward; `nil` for an action
  /// that answers instead, or a note that names no session.
  public static func shows(action: String, userInfo: [AnyHashable: Any]) -> String? {
    guard action == open || action == UNNotificationDefaultActionIdentifier else { return nil }
    return userInfo[sessionKey] as? String
  }

  /// Allow or Deny from the menu bar (T51.14): the same route a notification's Allow or Deny
  /// takes, so both answer an approval one way.
  public static func approval(session: String, call: String, allow: Bool) -> NotificationRoute? {
    route(
      action: allow ? Self.allow : Self.deny, userInfo: [sessionKey: session, callKey: call],
      text: nil)
  }

  static func title(_ kind: HostNote.Kind) -> String {
    switch kind {
    case .approval: "Approval needed"
    case .question: "Question"
    case .failed: "Turn failed"
    case .taskDone(let succeeded): succeeded ? "Task done" : "Task failed"
    }
  }
}

/// The notification centre's delegate: an action on a cox notification goes to `handle`, which
/// the app points at the session's store, a plugin's button goes to `act`, and Open or a click on
/// the notification itself goes to `show` with its session. The app keeps it alive and sets it as
/// `UNUserNotificationCenter.current().delegate` at launch.
public final class NotificationResponder: NSObject, UNUserNotificationCenterDelegate, Sendable {
  private let handle: @Sendable (NotificationRoute) -> Void
  private let show: @Sendable (String) -> Void
  private let act: @Sendable (PluginRoute) -> Void

  public init(
    handle: @escaping @Sendable (NotificationRoute) -> Void,
    show: @escaping @Sendable (String) -> Void = { _ in },
    act: @escaping @Sendable (PluginRoute) -> Void = { _ in }
  ) {
    (self.handle, self.show, self.act) = (handle, show, act)
  }

  public func userNotificationCenter(
    _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
  ) async {
    let text = (response as? UNTextInputNotificationResponse)?.userText
    let userInfo = response.notification.request.content.userInfo
    let plugin = NotificationActions.plugin(action: response.actionIdentifier, userInfo: userInfo)
    if let plugin { return act(plugin) }
    let route = NotificationActions.route(
      action: response.actionIdentifier, userInfo: userInfo, text: text)
    let shown = NotificationActions.shows(action: response.actionIdentifier, userInfo: userInfo)
    if let route {
      handle(route)
    } else if let shown {
      show(shown)
    }
  }

  /// A note that arrives while cox is frontmost still shows: the session it names may be in a
  /// window behind this one, and `notify` posts only what needs the person.
  public func userNotificationCenter(
    _ center: UNUserNotificationCenter, willPresent notification: UNNotification
  ) async -> UNNotificationPresentationOptions {
    [.banner, .list, .sound]
  }
}
