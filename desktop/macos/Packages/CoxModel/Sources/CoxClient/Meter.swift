// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The token meter's figures as `cox_app::MeterText` formats them (T37.25, A98): the popover's rows,
// the context split and the cache hit, field for field. Separate from `Timeline.swift`, which
// carries them inside `UsageView`, so the meter's text can grow without that file doing so.

/// `cox_app::MeterText`: every figure of the token meter and popover as it is shown.
public struct MeterText: Equatable, Sendable, Decodable {
  public var sent = "", received = "", rate = "", spoken = ""
  public var heading = "", phase = "", rateUnit = "", rateDetail = ""
  public var rows: [MeterRow] = []
  public var context = "", footnote = ""
  /// `7.6% of 1M` and `923.6k` free; empty while the window is unknown (A98).
  public var contextShare = "", contextFree = ""
  /// System, tools, instructions and history; empty until the first request.
  public var contextParts: [ContextPart] = []
  /// `94% this turn`; empty before a turn sent anything.
  public var cacheHit = ""
  /// `88% this session`, every call's; empty before the session sent anything (A104).
  public var cacheHitSession = ""
  /// `42%` without the window; the toolbar copies this so it does not split `contextShare`.
  public var contextPercent = ""
  /// Parts' shares already summed and capped at 1, so the toolbar does not add them.
  public var contextFill = 0.0
  /// Session cost as the Cost row already wrote it, so the toolbar does not reformat `costUsd`.
  public var cost = ""

  public init() {}

  enum CodingKeys: String, CodingKey {
    case sent, received, rate, spoken, heading, phase, rows, context, footnote
    case rateUnit = "rate_unit"
    case rateDetail = "rate_detail"
    case contextShare = "context_share"
    case contextFree = "context_free"
    case contextParts = "context_parts"
    case cacheHit = "cache_hit"
    case cacheHitSession = "cache_hit_session"
    case contextPercent = "context_percent"
    case contextFill = "context_fill"
    case cost
  }

  /// Recorded fixtures predate the toolbar fields (T58.4.15). A missing key
  /// is the empty figure, not a broken snapshot.
  public init(from decoder: Decoder) throws {
    let keys = try decoder.container(keyedBy: CodingKeys.self)
    sent = try keys.decodeIfPresent(String.self, forKey: .sent) ?? ""
    received = try keys.decodeIfPresent(String.self, forKey: .received) ?? ""
    rate = try keys.decodeIfPresent(String.self, forKey: .rate) ?? ""
    spoken = try keys.decodeIfPresent(String.self, forKey: .spoken) ?? ""
    heading = try keys.decodeIfPresent(String.self, forKey: .heading) ?? ""
    phase = try keys.decodeIfPresent(String.self, forKey: .phase) ?? ""
    rateUnit = try keys.decodeIfPresent(String.self, forKey: .rateUnit) ?? ""
    rateDetail = try keys.decodeIfPresent(String.self, forKey: .rateDetail) ?? ""
    rows = try keys.decodeIfPresent([MeterRow].self, forKey: .rows) ?? []
    context = try keys.decodeIfPresent(String.self, forKey: .context) ?? ""
    footnote = try keys.decodeIfPresent(String.self, forKey: .footnote) ?? ""
    contextShare = try keys.decodeIfPresent(String.self, forKey: .contextShare) ?? ""
    contextFree = try keys.decodeIfPresent(String.self, forKey: .contextFree) ?? ""
    contextParts = try keys.decodeIfPresent([ContextPart].self, forKey: .contextParts) ?? []
    cacheHit = try keys.decodeIfPresent(String.self, forKey: .cacheHit) ?? ""
    cacheHitSession = try keys.decodeIfPresent(String.self, forKey: .cacheHitSession) ?? ""
    contextPercent = try keys.decodeIfPresent(String.self, forKey: .contextPercent) ?? ""
    contextFill = try keys.decodeIfPresent(Double.self, forKey: .contextFill) ?? 0
    cost = try keys.decodeIfPresent(String.self, forKey: .cost) ?? ""
  }
}

/// `cox_app::ContextPart`: a part of the context window, its tokens and its share of the bar.
public struct ContextPart: Equatable, Sendable, Decodable {
  /// `system`, `tools`, `instructions` or `history`.
  public var kind, label, tokens: String
  public var share: Double

  public init(kind: String, label: String, tokens: String, share: Double) {
    (self.kind, self.label, self.tokens, self.share) = (kind, label, tokens, share)
  }
}

/// One line of the token popover's grid: a label and its turn and session figures.
public struct MeterRow: Equatable, Sendable, Decodable {
  public var label, turn, session: String
  public var detail: Bool

  public init(label: String, turn: String, session: String, detail: Bool) {
    (self.label, self.turn, self.session, self.detail) = (label, turn, session, detail)
  }
}
