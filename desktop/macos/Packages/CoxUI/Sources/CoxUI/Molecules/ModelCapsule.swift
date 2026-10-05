// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `ModelCapsule` (DS§6.3 row `ModelCapsule`, the mockup's first toolbar `.cap`): the session's
// model and effort, opening the model menu. Separate from `CostCapsule`: it shows a choice the
// person makes, not a figure that changes while they watch.

import SwiftUI

/// A sparkle, the model and effort as the core formats them, and a chevron, on a `CapsuleStyle`
/// capsule that turns active while its menu is open.
struct ModelCapsule: View {
  /// The model and effort, `Sonnet 5 · high`.
  let model: String
  let isOpen: Bool
  let action: () -> Void

  init(_ model: String, isOpen: Bool = false, action: @escaping () -> Void) {
    self.model = model
    self.isOpen = isOpen
    self.action = action
  }

  var body: some View {
    Button(action: action) {
      HStack(spacing: Space.s) {
        Image(systemName: "sparkle")
          .symbolStyle(.body)
          // The mockup's `--purple`, which the tokens name `role.project`; `status.plan` is
          // its `--blue`, the Plan segment's colour.
          .foregroundStyle(Color(.roleProject))
        Text(model).lineLimit(1)
        Image(systemName: "chevron.down").symbolStyle(.micro)
      }
    }
    .buttonStyle(CapsuleStyle(isOpen ? .active : .plain))
    .accessibilityLabel("Model")
    .accessibilityValue(model)
  }
}

#Preview("plain") { PreviewMatrix { ModelCapsule(PreviewState.model) {} } }
#Preview("open") { PreviewMatrix { ModelCapsule(PreviewState.model, isOpen: true) {} } }
