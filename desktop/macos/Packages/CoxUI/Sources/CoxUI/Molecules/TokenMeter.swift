// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TokenMeter` (DS§6.3 row `TokenMeter`, the mockup's `.meter`; DS§7; mockup screen 30): the
// session's ↑ sent and ↓ received tokens and the turn's live tok/s with its sparkline, in the
// composer's chip row, opening the token popover. Separate so the composer shows the figures
// from one value: every one arrives formatted by `cox-app`, and the meter only lays them out.

import SwiftUI

/// ↑ sent and ↓ received, then — once a rate is known — a StatusDot, tok/s and a Sparkline
/// behind a hairline, on a `CapsuleStyle` capsule that turns active while the popover is open.
/// VoiceOver reads the core's spoken line (DS§8).
public struct TokenMeter: View {
  /// The figures as `cox_app::MeterText` formats them, and the turn's tok/s series.
  public struct State: Equatable, Sendable {
    /// `218.5k`, `9.8k`, `71`; an empty rate hides the speed.
    public var sent = "", received = "", rate = ""
    /// "218 thousand tokens sent, 9.8 thousand received, 71 tokens per second".
    public var spoken = ""
    /// A turn runs: the dot glows; otherwise it rests as a ring.
    public var isStreaming = false
    /// Live tok/s, oldest first; only its shape is drawn.
    public var sparkline: [Double] = []

    public init() {}
  }

  let state: State
  let isOpen: Bool
  let action: () -> Void

  /// The mockup's meter sparkline, 44 × 16.
  private static let sparkWidth: CGFloat = 44
  private static let sparkHeight: CGFloat = 16

  public init(state: State, isOpen: Bool = false, action: @escaping () -> Void) {
    self.state = state
    self.isOpen = isOpen
    self.action = action
  }

  public var body: some View {
    Button(action: action) {
      HStack(spacing: Space.ml) {
        figure("↑", state.sent, "sent", tint: Color(.meterSent))
        figure("↓", state.received, "recv", tint: Color(.meterReceived))
        if !state.rate.isEmpty {
          HStack(spacing: Space.s) {
            StatusDot(state.isStreaming ? .running : .idle)
            figure(nil, state.rate, "tok/s", tint: .clear)
            Sparkline(state.sparkline)
              .frame(width: Self.sparkWidth, height: Self.sparkHeight)
          }
          .padding(.leading, Space.ml)
          .hairline(.leading)
        }
      }
    }
    .buttonStyle(CapsuleStyle(isOpen ? .active : .plain))
    .help("Tokens this session")
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(accessibilityText)
    .accessibilityAddTraits(.isButton)
  }

  /// What VoiceOver reads for the whole meter: the core's spoken line (DS§8).
  var accessibilityText: String { state.spoken }

  /// An arrow in its meter colour, the figure in `text.primary`, its unit in `text.secondary`.
  private func figure(_ arrow: String?, _ value: String, _ unit: String, tint: Color) -> some View {
    HStack(spacing: Space.xxs) {
      if let arrow {
        Text(arrow).fontWeight(.semibold).foregroundStyle(tint)
      }
      Text(value).fontWeight(.semibold).foregroundStyle(Color(.textPrimary))
      Text(unit).foregroundStyle(Color(.textSecondary))
    }
    .textStyle(.footnote, tabularDigits: true)
  }
}

#Preview("idle") { PreviewMatrix { TokenMeterSample(state: PreviewState.meterIdle) } }
#Preview("streaming") { PreviewMatrix { TokenMeterSample(state: PreviewState.meterStreaming) } }
#Preview("open") {
  PreviewMatrix { TokenMeterSample(state: PreviewState.meterStreaming, isOpen: true) }
}
