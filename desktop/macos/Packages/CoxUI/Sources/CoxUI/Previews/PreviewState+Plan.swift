// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `PreviewState` fixtures for the inspector's Plan tab (T37.29.2): the mockup's five steps — two
// done, one in progress, two pending. Separate from `PreviewState+Inspector.swift` so the
// inspector's tabs, built in parallel, add their fixtures without editing one file.

import SwiftUI

extension PreviewState {
  /// The mockup's Plan tab.
  static let plan = PlanTab.State(items: [
    .init(id: "1", text: "Read retry.rs and callers", step: .done),
    .init(id: "2", text: "Switch delay() to full jitter with a cap", step: .done),
    .init(id: "3", text: "Add delay_never_exceeds_cap test", step: .inProgress),
    .init(id: "4", text: "Run cox-provider-http tests", step: .pending),
    .init(id: "5", text: "Push branch and open PR", step: .pending),
  ])
}

/// The inspector on its Plan tab, as tall as the smallest window.
struct PlanInspectorSample: View {
  let state: PlanTab.State

  var body: some View {
    Inspector(selection: .plan, content: PlanTab(state: state)) { _ in }
      .frame(height: Size.windowMinHeight)
  }
}
