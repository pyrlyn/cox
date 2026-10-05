// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `CostCapsule` (DS§6.3 row `CostCapsule`, the mockup's toolbar `.cap` with `.ring`): what the
// session has cost and how full its context is, opening the token popover. Separate from
// `ModelCapsule`: its figures change while you watch, so its digits are tabular.

import SwiftUI

/// A `ProgressRing` of the context share, the cost, then the share in `text.secondary`, on a
/// `CapsuleStyle` capsule that turns active while the popover is open.
struct CostCapsule: View {
  /// What the core formatted: `$0.42` and `38%`.
  let cost: String
  let context: String
  /// The context share the ring fills, 0…1.
  let fraction: Double
  let isOpen: Bool
  let action: () -> Void

  init(
    cost: String, context: String, fraction: Double, isOpen: Bool = false,
    action: @escaping () -> Void
  ) {
    self.cost = cost
    self.context = context
    self.fraction = fraction
    self.isOpen = isOpen
    self.action = action
  }

  var body: some View {
    Button(action: action) {
      HStack(spacing: Space.s) {
        ProgressRing(fraction)
        Text(cost).textStyle(.control, tabularDigits: true)
        Text("· ctx \(context)")
          .textStyle(.compact, tabularDigits: true)
          .foregroundStyle(Color(.textSecondary))
      }
    }
    .buttonStyle(CapsuleStyle(isOpen ? .active : .plain))
    .accessibilityLabel("Cost \(cost), context \(context) used")
  }
}

#Preview("plain") { PreviewMatrix { CostCapsuleSample(isOpen: false) } }
#Preview("open") { PreviewMatrix { CostCapsuleSample(isOpen: true) } }
