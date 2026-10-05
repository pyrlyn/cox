// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `SettingsSidebar` (DS§6.4 row `SettingsSidebar`, the mockup's `.set-side`; DT§5.7): the
// Settings window's search field, the page list — General through Advanced — and, at its foot,
// the config files the values come from. Separate so the Settings screen composes its sidebar
// like `MainScreen` composes the session list, and reports a page choice and a query as intents.

import SwiftUI

/// The Settings pages in DT§5.7's order, each with its title and DS§3.7 symbol.
public enum SettingsPage: String, CaseIterable, Identifiable, Sendable {
  case general, models, permissions, sandbox, budget, mcp, plugins, appearance, advanced

  public var id: Self { self }

  var title: String {
    switch self {
    case .general: "General"
    case .models: "Models & Providers"
    case .permissions: "Permissions"
    case .sandbox: "Sandbox"
    case .budget: "Budget"
    case .mcp: "MCP Servers"
    case .plugins: "Plugins"
    case .appearance: "Appearance"
    case .advanced: "Advanced"
    }
  }

  var symbol: String {
    switch self {
    case .general: "gearshape"
    case .models: "sparkle"
    case .permissions: "shield"
    case .sandbox: "lock"
    case .budget: "dollarsign.circle"
    case .mcp: "powerplug"
    case .plugins: "cpu"
    case .appearance: "paintbrush"
    case .advanced: "slider.horizontal.3"
    }
  }

  /// The symbol on the page's `tile.settings.<page>` tile, a macOS system colour (A96).
  var tile: IconTile {
    switch self {
    case .general:
      tile(.tileSettingsGeneralTop, .tileSettingsGeneralBottom, .tileSettingsGeneralGlyph)
    case .models: tile(.tileSettingsModelsTop, .tileSettingsModelsBottom, .tileSettingsModelsGlyph)
    case .permissions:
      tile(
        .tileSettingsPermissionsTop, .tileSettingsPermissionsBottom, .tileSettingsPermissionsGlyph)
    case .sandbox:
      tile(.tileSettingsSandboxTop, .tileSettingsSandboxBottom, .tileSettingsSandboxGlyph)
    case .budget: tile(.tileSettingsBudgetTop, .tileSettingsBudgetBottom, .tileSettingsBudgetGlyph)
    case .mcp: tile(.tileSettingsMcpTop, .tileSettingsMcpBottom, .tileSettingsMcpGlyph)
    case .plugins:
      tile(.tileSettingsPluginsTop, .tileSettingsPluginsBottom, .tileSettingsPluginsGlyph)
    case .appearance:
      tile(.tileSettingsAppearanceTop, .tileSettingsAppearanceBottom, .tileSettingsAppearanceGlyph)
    case .advanced:
      tile(.tileSettingsAdvancedTop, .tileSettingsAdvancedBottom, .tileSettingsAdvancedGlyph)
    }
  }

  private func tile(
    _ top: ColorResource, _ bottom: ColorResource, _ glyph: ColorResource
  ) -> IconTile {
    IconTile(face: [Color(top), Color(bottom)], glyph: Color(glyph), symbol: symbol)
  }
}

/// A `SessionFilter` over `pages` as `InspectorRow`s led by their tiles on a
/// `ShellPane(.sidebar)`, the selected one lifted on `accent` in `text.onAccent`, over the user
/// file and the project file with the layer badge each one sets.
struct SettingsSidebar: View {
  let pages: [SettingsPage]
  let selection: SettingsPage
  let userFile: String
  let projectFile: String?
  /// The search; `pages` arrive already narrowed to it, and each title marks where it matches.
  var filter = ""
  var search: (String) -> Void = { _ in }
  let select: (SettingsPage) -> Void

  var body: some View {
    ShellPane(.sidebar) {
      VStack(alignment: .leading, spacing: 0) {
        // The system's window buttons sit in this row.
        Spacer().frame(height: Size.toolbarHeight - Size.paneGap)
        // The mockup's `.filter` 2 px inside the rows' 8 px sides: `Space.ml`.
        SessionFilter(text: Binding(get: { filter }, set: search), prompt: "Search settings")
          .onExitCommand { if !filter.isEmpty { search("") } }
          .padding(.horizontal, Space.ml)
          .padding(.bottom, Space.ml)
        ScrollView {
          VStack(spacing: Space.xxs) {
            ForEach(pages) { page in
              Button {
                select(page)
              } label: {
                // The mockup's `.set-side .it.on`: the chosen page on `accent`, as macOS
                // Settings marks it, rather than a session row's paler selection.
                InspectorRow(glyph: page.tile, isSelected: false, actions: []) {
                  // On the selected row's `accent` a match is bold alone: a soft mark would
                  // sink `text.onAccent` below DS§8.
                  Text(
                    AttributedString(
                      page.title, marking: filter,
                      on: page == selection ? nil : Color(.accentSoft))
                  )
                  .foregroundStyle(Color(page == selection ? .textOnAccent : .textPrimary))
                  .frame(maxWidth: .infinity, alignment: .leading)
                }
                .rowSelection(page == selection, fill: .accent)
              }
              .buttonStyle(.plain)
            }
          }
          .padding(.horizontal, Space.m)
        }
        VStack(alignment: .leading, spacing: Space.xs) {
          ConfigFile(path: userFile, source: .user)
          if let projectFile { ConfigFile(path: projectFile, source: .project) }
        }
        .padding(Space.l)
        .hairline(.top)
      }
    }
    .frame(width: Size.sidebarWidth)
  }
}

/// A config file's path, cut in the middle, and the badge of the layer it sets.
private struct ConfigFile: View {
  let path: String
  let source: SettingSource

  var body: some View {
    HStack(spacing: Space.s) {
      Text(path)
        .textStyle(.footnote)
        .foregroundStyle(Color(.textSecondary))
        .lineLimit(1)
        .truncationMode(.middle)
        .frame(maxWidth: .infinity, alignment: .leading)
      Badge(source.name, kind: source.badge)
    }
    .accessibilityElement(children: .combine)
  }
}

#Preview("models") {
  PreviewMatrix {
    SettingsSidebar(
      pages: SettingsPage.allCases, selection: .models, userFile: PreviewState.userFile,
      projectFile: PreviewState.projectFile
    ) { _ in }
    .frame(height: Size.windowMinHeight)
  }
}

#Preview("filtered") {
  PreviewMatrix {
    SettingsSidebar(
      pages: PreviewState.settingsFiltered.pages, selection: .models,
      userFile: PreviewState.userFile, projectFile: PreviewState.projectFile,
      filter: PreviewState.settingsFiltered.filter
    ) { _ in }
    .frame(height: Size.windowMinHeight)
  }
}

#Preview("models, high contrast") {
  PreviewMatrix {
    SettingsSidebar(
      pages: SettingsPage.allCases, selection: .models, userFile: PreviewState.userFile,
      projectFile: PreviewState.projectFile
    ) { _ in }
    .frame(height: Size.windowMinHeight)
    .environment(\._colorSchemeContrast, .increased)
  }
}
