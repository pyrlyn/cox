// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TranscriptView` (DS§6.4 row `TranscriptView`, DT§5.2): a session's timeline as one
// selectable document — `CoxTranscriptText`'s TextKit 2 view with the blocks as text and the
// card blocks as CoxUI cards — kept in step with the session by the same patches its
// `SessionStore` applies, so a streamed reply edits only its own range. Separate as the one
// SwiftUI view that hosts the AppKit transcript; what it draws comes from `TranscriptCard` and
// `TranscriptStyle.cox`.

import AppKit
import CoxClient
import CoxModel
import CoxTranscriptText
import CoxUI
import SwiftUI

/// The transcript of `store`'s session. `crossBlockSelection` is the app's
/// `[desktop.transcript] cross_block_selection` (A67): on, a drag runs across blocks like a
/// document; off, it stays in the block it started in. Approvals and questions show in the
/// `approval` slot.
public struct TranscriptView<Approval: View>: NSViewRepresentable {
  let store: SessionStore
  let crossBlockSelection: Bool
  let approval: @MainActor (Block) -> Approval
  /// Where a prompt's Edit and resend puts its text (`composer(_:)`).
  var composer: ComposerStore?
  private var textSize = Double(FontToken.transcript.size)
  private var lineHeight = Double(FontToken.transcript.lineHeight)

  public init(
    store: SessionStore, crossBlockSelection: Bool = true,
    @ViewBuilder approval: @escaping @MainActor (Block) -> Approval
  ) {
    self.store = store
    self.crossBlockSelection = crossBlockSelection
    self.approval = approval
  }

  /// The prose at `[desktop.transcript]`'s `text_size`, in pt at 100 % text size, and
  /// `line_height`, a multiple of that size (A93); unset, the `font.transcript` token's.
  public func text(size: Double, lineHeight: Double) -> Self {
    var copy = self
    (copy.textSize, copy.lineHeight) = (size, lineHeight)
    return copy
  }

  /// What the text is styled from: the appearance it is drawn at, Reduce Transparency forcing
  /// Solid and the colour scheme picking the dark highlight as for any view (A109), with the
  /// configured text size and line height.
  private func styling(_ environment: EnvironmentValues) -> TextStyling {
    TextStyling(
      appearance: environment.coxAppearance.effective(
        reduceTransparency: environment.accessibilityReduceTransparency,
        colorScheme: environment.colorScheme),
      textSize: textSize, lineHeight: lineHeight)
  }

  public func makeCoordinator() -> TranscriptCoordinator { TranscriptCoordinator() }

  public func makeNSView(context: Context) -> NSScrollView {
    let appearance = context.environment.coxAppearance
    let shared = context.coordinator.appearance
    shared.value = appearance
    shared.locale = context.environment.locale
    let styling = styling(context.environment)
    let text = TranscriptTextView.make(style: styling.style)
    context.coordinator.styling = styling
    text.cards = TranscriptCards { [approval] block in
      CardAppearance(shared: shared) { TranscriptCard(block: block, approval: approval) }
    } thumbnail: { name in
      CardAppearance(shared: shared) { Thumbnail(name) }
    } thinking: { title, open, toggle in
      CardAppearance(shared: shared) {
        ThinkingHeader(title, isExpanded: open, action: toggle)
      }
    }
    offerPromptActions(on: text, shared)
    text.crossBlockSelection = crossBlockSelection
    text.drawsBackground = false
    let scroll = text.inScrollView(frame: .zero)
    scroll.drawsBackground = false
    // The pane sits below the toolbar, yet AppKit would pad it by the full-size content's title
    // bar overlap, and `TailFollow`'s scroll to the end, which counts no content inset, would
    // then hide the first line's top under that padding.
    scroll.automaticallyAdjustsContentInsets = false
    text.load(store.blocks.values)
    context.coordinator.follow(store, into: text)
    return scroll
  }

  public func updateNSView(_ scroll: NSScrollView, context: Context) {
    let text = scroll.documentView as? TranscriptTextView
    text?.crossBlockSelection = crossBlockSelection
    text.map { offerPromptActions(on: $0, context.coordinator.appearance) }
    let appearance = context.environment.coxAppearance
    context.coordinator.appearance.value = appearance
    context.coordinator.appearance.locale = context.environment.locale
    text.map { context.coordinator.restyle($0, to: styling(context.environment)) }
  }

  public static func dismantleNSView(_ scroll: NSScrollView, coordinator: TranscriptCoordinator) {
    coordinator.stop()
  }
}

/// What the transcript keeps across SwiftUI updates: the store it follows and the appearance
/// its cards read, which reaches them here because a hosted card is not in SwiftUI's tree.
@MainActor
public final class TranscriptCoordinator {
  let appearance = SharedAppearance()
  private weak var store: SessionStore?
  private var tail: TailFollow?
  /// What the transcript was last styled from.
  var styling: TextStyling?

  /// Splices each batch the store applies into `text`, after the store, so a block `current`
  /// returns is as the batch left it, keeping the view at the end while the reader is there
  /// (`TailFollow`).
  func follow(_ store: SessionStore, into text: TranscriptTextView) {
    self.store = store
    let tail = TailFollow(text)
    self.tail = tail
    store.didApply = { [weak text, weak store] patches in
      tail.around { text?.apply(patches) { store?.blocks[$0] } }
    }
  }

  func stop() { store?.didApply = nil }

  /// Restyles `text` at a new text size or line height (T37.23.6, A93) or bubble look —
  /// material, Depth or opacity (T37.23.9) — staying at the end if the reader was there. Keyed
  /// on the styling, so a SwiftUI update that leaves it alone builds no style, and one that
  /// leaves the style alone restyles nothing.
  func restyle(_ text: TranscriptTextView, to styling: TextStyling) {
    guard styling != self.styling, let tail else { return }
    self.styling = styling
    let style = styling.style
    guard style != text.style else { return }
    tail.around(restyling: true) { text.restyle(style) }
  }
}

/// What styles the transcript's text: the appearance it is drawn at, whose `textScale` is
/// ⌘+/⌘−'s, and `[desktop.transcript]`'s `text_size` and `line_height` (A93).
struct TextStyling: Equatable {
  let appearance: Appearance
  let textSize: Double
  let lineHeight: Double

  @MainActor var style: TranscriptStyle {
    .cox(
      appearance, textScale: appearance.textScale * textSize / FontToken.transcript.size,
      lineHeight: lineHeight)
  }
}

/// The `coxAppearance` and locale the transcript was given, observed by every card it hosts.
@Observable
@MainActor
final class SharedAppearance {
  var value = Appearance()
  var locale = Locale.current
}

/// A hosted card with the transcript's appearance and locale.
struct CardAppearance<Content: View>: View {
  let shared: SharedAppearance
  @ViewBuilder let content: Content

  var body: some View {
    content.environment(\.coxAppearance, shared.value).environment(\.locale, shared.locale)
  }
}

extension TranscriptStyle {
  /// The transcript drawn with CoxUI's tokens: `font.transcript` prose, `font.transcript.h1`,
  /// `h3` and `h4` headings (A94; one line height, the three tokens share it) and
  /// `font.mono.code` code at the user's text size, each at its token's line
  /// height but the prose at `lineHeight` (A93), lists indented as the
  /// mockup's (`space.xxl`), readable colours only (DS§8) — the status colours miss
  /// 4.5:1 as text, so `ok`, `warn`, `error` and the diff tokens keep `text.primary`. A prompt
  /// sits on `UserBubble`'s face and a thought reads as `ThinkingDisclosure` (T37.21.5); a
  /// quote's bars are `quote.bar`, `size.quoteBar` wide (A97).
  /// `appearance` is what the text is drawn at, Reduce Transparency applied.
  @MainActor
  static func cox(
    _ appearance: Appearance, textScale: Double,
    lineHeight: Double = FontToken.transcript.lineHeight
  ) -> TranscriptStyle {
    let secondary = TextColour.secondary.nsColor
    let bubble = TranscriptStyle.Bubble.cox(appearance)
    return TranscriptStyle(
      body: FontToken.transcript.nsFont(scale: textScale),
      code: FontToken.monoCode.nsFont(scale: textScale),
      headings: .init(
        h1: FontToken.transcriptH1.nsFont(scale: textScale),
        h3: FontToken.transcriptH3.nsFont(scale: textScale),
        h4: FontToken.transcriptH4.nsFont(scale: textScale)),
      text: TextColour.primary.nsColor,
      colors: [
        .dim: secondary, .tool: secondary, .diffHunk: secondary,
        .accent: TextColour.accent.nsColor, .border: TextColour.tertiary.nsColor,
      ],
      // A prompt's bubble reaches its padding above the text; the mockup keeps `Space.xxl`
      // clear between the pane's top edge and the first bubble.
      blockSpacing: Space.l,
      inset: NSSize(width: Space.xl, height: Space.xxl + bubble.padding.height), bubble: bubble,
      thought: .init(
        font: NSFontManager.shared.convert(
          FontToken.caption.nsFont(scale: textScale), toHaveTrait: .italicFontMask),
        color: secondary, rule: SurfaceColour.separator.nsColor, ruleWidth: Size.hairline,
        indent: Space.l),
      quote: .init(bar: SurfaceColour.quoteBar.nsColor, barWidth: Size.quoteBar),
      indent: Space.xxl,
      lineHeights: .init(
        body: lineHeight, code: FontToken.monoCode.lineHeight,
        heading: FontToken.transcriptH3.lineHeight, thought: FontToken.caption.lineHeight),
      readingWidth: Size.readingWidth,
      // CoxUI's `TurnGutter` (Figma frames 01, 14): the turn number left of a prompt (T37.47).
      gutter: .init(
        font: FontToken.detail.nsFont(scale: textScale), color: secondary,
        width: Size.turnGutter, offset: Size.turnGutterOffset))
  }
}

extension TranscriptStyle.Bubble {
  /// `UserBubble` as AppKit draws it (T37.23.9): `fill.primary` on the readable face, the glass
  /// sweep over it and e2 under it, from the same tokens and at the same appearance.
  @MainActor
  static func cox(_ appearance: Appearance) -> Self {
    Self(
      fill: SurfaceColour.fillPrimary.nsColor, radius: Radius.xl,
      padding: NSSize(width: Space.l, height: Space.ml), gap: Space.m,
      face: SurfaceColour.window.nsColor.withAlphaComponent(appearance.readableOpacity),
      sweep: appearance.sweepStops.map {
        TranscriptStyle.SweepStop(
          color: SurfaceColour.specular.nsColor.withAlphaComponent($0.opacity),
          location: $0.location)
      },
      shadows: ElevationToken.e2.layers(at: appearance).map {
        TranscriptStyle.Shadow(
          color: NSColor($0.color), offset: CGSize(width: $0.x, height: $0.y), blur: $0.blur,
          spread: $0.spread, inset: $0.inset)
      })
  }
}
