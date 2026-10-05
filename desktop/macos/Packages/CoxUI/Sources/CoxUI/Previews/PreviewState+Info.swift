// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the inspector's Info tab (T37.29.5): a session in a linked worktree
// with a user and a project config. Separate from `PreviewState+Inspector.swift` so the
// inspector's tabs, built in parallel, add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  /// The session's facts.
  static let infoSession: [KeyValueGrid.Row] = [
    .init(label: "Session", values: ["01J9ZK4QX7M3T8V2B6N0PDRWYE"]),
    .init(label: "Folder", values: ["~/GitHub/cox"]),
    .init(label: "Worktree", values: ["~/GitHub/_worktrees/cox-t1"]),
    .init(label: "Branch", values: ["wt/retry-jitter"], isDetail: true),
    .init(label: "Rollout", values: ["~/.cox/sessions/01J9ZK4QX7M3T8V2B6N0PDRWYE.jsonl"]),
  ]

  /// The config layers, each file under its layer.
  static let infoConfig: [KeyValueGrid.Row] = [
    .init(label: "default", values: ["112 keys"]),
    .init(label: "user", values: ["3 keys"]),
    .init(label: "~/.cox/config.toml", values: [], isDetail: true),
    .init(label: "project", values: ["1 key"]),
    .init(label: "~/GitHub/cox/.cox/config.toml", values: [], isDetail: true),
  ]

  static let info = InfoTab.State(session: infoSession, config: infoConfig)
}

/// The inspector on its Info tab, as tall as the smallest window.
struct InfoInspectorSample: View {
  let state: InfoTab.State

  var body: some View {
    Inspector(selection: .info, content: InfoTab(state: state)) { _ in }
      .frame(height: Size.windowMinHeight)
  }
}
