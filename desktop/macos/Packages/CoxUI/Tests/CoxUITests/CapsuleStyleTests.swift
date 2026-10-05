// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CapsuleStyle`'s check (T37.19.2): one snapshot per emphasis (plain, active) × light/dark ×
// Solid/Frosted, each showing rest, hovered, pressed and disabled.

import SwiftUI
import Testing

@testable import CoxUI

@MainActor
@Suite struct CapsuleStyleSnapshotTests {
  @Test(arguments: CapsuleStyle.Emphasis.allCases, Variant.all)
  func capsule(_ emphasis: CapsuleStyle.Emphasis, _ variant: Variant) throws {
    try assertCoxSnapshot(
      CapsuleSample(emphasis: emphasis), variant, named: "\(emphasis)-\(variant.name)")
  }
}

/// One emphasis in every state, on a pane as capsules sit in the toolbar.
private struct CapsuleSample: View {
  let emphasis: CapsuleStyle.Emphasis

  var body: some View {
    PreviewPane {
      HStack(spacing: Space.l) {
        ForEach(ControlState.allCases, id: \.self) { state in
          CapsuleFace(label: Text("claude-opus-5"), emphasis: emphasis, state: state)
        }
      }
    }
  }
}
