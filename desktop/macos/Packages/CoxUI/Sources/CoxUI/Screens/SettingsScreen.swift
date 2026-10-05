// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SettingsScreen` (DS§6.5; DT§5.7): the Settings window — the page list, and the selected
// page's config tables as boxes of `SettingRow`s, each value with the layer it comes from, and
// a key row in each provider's box (`SettingsScreen+Keys.swift`). Composition only (DS§5): what each field shows,
// whether it is read-only and which file sets it arrive in `state`; the app binds `state` and
// `send` to `SettingsStore`, through the public types below.

import SwiftUI

/// The pages that hold settings, the selected one and its tables, and the files behind them.
public struct SettingsScreenState: Equatable, Sendable {
  public var pages: [SettingsPage] = []
  public var selection = SettingsPage.general
  /// The selected page's tables, in key order.
  public var tables: [SettingsScreen.Table] = []
  public var userFile = ""
  public var projectFile: String?
  /// The MCP servers' logins, on the MCP page.
  public var logins: [SettingsScreen.Login] = []
  /// The page's project values the guard list threw out.
  public var dropped: [SettingsScreen.DroppedValue] = []
  /// The sidebar's search; `pages` and `tables` arrive narrowed to it, and labels mark it.
  public var filter = ""
  /// The rules and session grants, on the Permissions page.
  public var permissions: SettingsScreen.Permissions?
  /// The global shortcuts the person records, on the General page (T51.15).
  public var shortcuts: [SettingsScreen.Shortcut] = []

  public init(
    pages: [SettingsPage] = [], selection: SettingsPage = .general,
    tables: [SettingsScreen.Table] = [], userFile: String = "", projectFile: String? = nil,
    logins: [SettingsScreen.Login] = [], dropped: [SettingsScreen.DroppedValue] = [],
    filter: String = "",
    permissions: SettingsScreen.Permissions? = nil,
    shortcuts: [SettingsScreen.Shortcut] = []
  ) {
    (self.pages, self.selection, self.tables) = (pages, selection, tables)
    (self.userFile, self.projectFile) = (userFile, projectFile)
    (self.logins, self.dropped, self.filter) = (logins, dropped, filter)
    (self.permissions, self.shortcuts) = (permissions, shortcuts)
  }
}

/// Every intent the Settings screen reports.
public enum SettingsScreenIntent: Equatable, Sendable {
  case select(SettingsPage)
  /// The search typed in the sidebar; Esc sends an empty one.
  case filter(String)
  /// A new value for `key`; a slider reports it while it moves.
  case set(key: String, SettingsScreen.Edit)
  /// A key typed for a provider section, bound for its `SecretStore`.
  case storeKey(provider: String, secret: String)
  /// Log in to (`true`) or out of an MCP server.
  case setLogin(server: String, Bool)
  /// Add (`old` nil), replace or remove (`new` nil) one permission rule in the user config.
  case editRule(SettingsScreen.RuleKind, old: String?, new: String?)
  /// Revoke the session grant with this id.
  case revokeGrant(id: String)
}

/// The Settings window: `SettingsSidebar` beside the selected page's title over a column of
/// `SettingsGroupBox`es.
public struct SettingsScreen: View {
  let state: SettingsScreenState
  let send: (SettingsScreenIntent) -> Void
  /// The control that records a shortcut, by its id: the app's, from its hotkey library, which
  /// CoxUI does not link (T51.15).
  let recorder: (@MainActor (SettingsScreen.Shortcut.ID) -> AnyView)?

  public init(
    state: SettingsScreenState,
    recorder: (@MainActor (SettingsScreen.Shortcut.ID) -> AnyView)? = nil,
    send: @escaping (SettingsScreenIntent) -> Void
  ) {
    (self.state, self.recorder, self.send) = (state, recorder, send)
  }

  public var body: some View {
    ShellPane(.window) {
      HStack(spacing: Size.paneGap) {
        SettingsSidebar(
          pages: state.pages, selection: state.selection, userFile: state.userFile,
          projectFile: state.projectFile, filter: state.filter, search: { send(.filter($0)) },
          select: { send(.select($0)) })
        ShellPane(.column) {
          ScrollView {
            VStack(alignment: .leading, spacing: Space.xl) {
              // The mockup's `.set-main h1`.
              Text(state.selection.title)
                .textStyle(.titlePage)
                .foregroundStyle(Color(.textPrimary))
                .accessibilityAddTraits(.isHeader)
              if !state.dropped.isEmpty { DroppedBox(values: state.dropped) }
              if !state.logins.isEmpty { LoginsBox(logins: state.logins, send: send) }
              if !state.shortcuts.isEmpty, let recorder {
                ShortcutsBox(shortcuts: state.shortcuts, recorder: recorder)
              }
              ForEach(state.tables) { TableBox(table: $0, send: send) }
              if let permissions = state.permissions {
                PermissionsBoxes(permissions: permissions, send: send)
              }
            }
            .frame(maxWidth: Size.readingWidth)
            .padding(Space.xxl)
            .frame(maxWidth: .infinity)
          }
          .environment(\.settingsFilter, state.filter)
        }
      }
      .padding(Size.paneGap)
    }
  }
}

extension SettingsScreen {
  /// One config table: its settings and, for a provider's table, the key field.
  public struct Table: Identifiable, Equatable, Sendable {
    /// The keys' shared prefix, `tiers.code`; the box's header.
    public let id: String
    var fields: [Field]
    var key: Key?

    public init(id: String, fields: [Field], key: Key? = nil) {
      (self.id, self.fields, self.key) = (id, fields, key)
    }
  }

  public struct Field: Identifiable, Equatable, Sendable {
    /// The dotted config key.
    public let id: String
    var title: String
    /// The schema's help text, or the file a read-only value is set in.
    var detail: String?
    var source: SettingSource
    var control: Control

    public init(
      id: String, title: String, detail: String?, source: SettingSource, control: Control
    ) {
      (self.id, self.title, self.detail, self.source, self.control) = (
        id, title, detail, source, control
      )
    }
  }

  /// A provider section's key: whether one is stored, never the key itself.
  public struct Key: Equatable, Sendable {
    var provider: String
    var isStored: Bool

    public init(provider: String, isStored: Bool) {
      (self.provider, self.isStored) = (provider, isStored)
    }
  }

  public enum Control: Equatable, Sendable {
    case toggle(Bool)
    case slider(Double, range: ClosedRange<Double>, text: String)
    /// A few options, side by side.
    case choice(String, options: [String])
    /// A pop-up of more options, each with the title the menu shows.
    case menu(String, options: [Option])
    case field(String)
    /// A value Settings shows but does not edit.
    case json(String)
  }

  public struct Option: Hashable, Sendable {
    /// What a pick sends.
    var value: String
    var title: String

    public init(value: String, title: String) { (self.value, self.title) = (value, title) }
  }

  public enum Edit: Equatable, Sendable {
    case bool(Bool)
    case number(Double)
    /// Typed or chosen text; the store types it by the field's kind.
    case text(String)
  }
}

/// A table's box: the key field first, then a row per setting.
private struct TableBox: View {
  let table: SettingsScreen.Table
  let send: (SettingsScreenIntent) -> Void

  var body: some View {
    SettingsGroupBox(table.id) {
      if let key = table.key { KeyRow(key: key, send: send) }
      ForEach(table.fields) { FieldRow(field: $0, send: send) }
    }
  }
}

/// One setting in the row its control takes.
private struct FieldRow: View {
  let field: SettingsScreen.Field
  let send: (SettingsScreenIntent) -> Void

  var body: some View {
    switch field.control {
    case .toggle(let isOn):
      SettingRow(source: field.source) {
        LabeledToggle(field.title, detail: field.detail, isOn: binding(isOn) { .bool($0) })
      }
    case .slider(let value, let range, let text):
      SettingRow(source: field.source) {
        LabeledSlider(
          field.title, value: binding(value) { .number($0) }, in: range, valueText: text)
      }
    case .choice(let selection, let options):
      SettingRow(field.title, detail: field.detail, source: field.source) {
        CoxSegmented(
          LocalizedStringKey(field.title), selection: binding(selection) { .text($0) },
          options: options, title: { Text($0) })
      }
    case .menu(let selection, let options):
      SettingRow(field.title, detail: field.detail, source: field.source) {
        SettingPopUp(
          LocalizedStringKey(field.title), selection: binding(selection) { .text($0) },
          options: options.map(\.value),
          title: { value in options.first { $0.value == value }?.title ?? value })
      }
    case .field(let text):
      SettingRow(field.title, detail: field.detail, source: field.source) {
        SettingField(text, prompt: field.title) { send(.set(key: field.id, .text($0))) }
      }
    case .json(let text):
      SettingRow(field.title, detail: field.detail, source: field.source) {
        SettingField(text, prompt: field.title) { _ in }.disabled(true)
      }
    }
  }

  private func binding<Value>(
    _ value: Value, _ edit: @escaping (Value) -> SettingsScreen.Edit
  ) -> Binding<Value> {
    Binding(get: { value }, set: { send(.set(key: field.id, edit($0))) })
  }
}

#Preview("models") {
  SettingsScreen(state: PreviewState.settingsModels) { _ in }
    .frame(width: Size.windowMinWidth, height: Size.windowMinHeight)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}

#Preview("filtered") {
  SettingsScreen(state: PreviewState.settingsFiltered) { _ in }
    .frame(width: Size.windowMinWidth, height: Size.windowMinHeight)
    .padding(Space.xxl)
    .background(PreviewBackdrop())
}
