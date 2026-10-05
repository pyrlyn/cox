// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Generated cox-ffi values ⇄ `CoxClient` values, field for field. The two
// sets have the same shape (both mirror cox-app's serde types); this file
// keeps the generated ones inside CoxCore, so the stores and views never
// link Rust. Exhaustive switches: a variant added in Rust fails this build.

import CoxClient
import CoxFFIBindings

extension CoxClient.Block {
  init(_ value: CoxFFIBindings.Block) {
    self.init(id: value.id, turn: value.turn, kind: .init(value.kind))
  }
}

extension CoxClient.BlockKind {
  init(_ value: CoxFFIBindings.BlockKind) {
    switch value {
    case .user(let text, let attachments): self = .user(text: text, attachments: attachments)
    case .assistant(let text, let doc, let pluginView):
      self = .assistant(
        text: text, doc: .init(doc), pluginView: pluginView.map { CoxClient.PluginView($0) })
    case .thinking(let text, let durationMs):
      self = .thinking(text: text, durationMs: durationMs)
    case .tool(
      let tool, let summary, let icon, let risk, let state, let tail, let archive, let diff,
      let durationMs, let pluginView):
      self = .tool(
        tool: tool, summary: summary, icon: .init(icon), risk: .init(risk), state: .init(state),
        tail: tail, archive: archive.map { .init(id: $0.id, bytes: $0.bytes) },
        diff: diff.map { CoxClient.DiffModel($0) }, durationMs: durationMs,
        pluginView: pluginView.map { CoxClient.PluginView($0) })
    case .toolGroup(let summary, let children, let state):
      self = .toolGroup(summary: summary, children: children, state: .init(state))
    case .approval(
      let call, let tool, let summary, let input, let grants, let why, let source, let decision,
      let decidedBy):
      self = .approval(
        call: call, tool: tool, summary: summary, input: input, grants: grants, why: .init(why),
        source: source.map { .init(session: $0.session, agent: $0.agent, preset: $0.preset) },
        decision: decision.map { CoxClient.Decision($0) },
        by: decidedBy.map { CoxClient.DecidedBy($0) })
    case .question(let call, let question, let options, let answer):
      self = .question(call: call, question: question, options: options, answer: answer)
    case .task(
      let task, let label, let tier, let done, let costUsd, let exitCode, let state, let kind):
      self = .task(
        task: task, label: label, tier: .init(tier), done: done, costUsd: costUsd,
        exitCode: exitCode, state: .init(state), kind: .init(kind))
    case .compaction(let before, let after, let reason, let summary):
      self = .compaction(
        beforeTokens: before, afterTokens: after, reason: .init(reason), summary: summary)
    case .checkpoint(let files): self = .checkpoint(files: files)
    case .notice(let level, let text): self = .notice(level: .init(level), text: text)
    case .error(let text, let fatal): self = .error(text: text, fatal: fatal)
    case .turnMeta(let model, let tier, let usage, let stop):
      self = .turnMeta(
        model: model, tier: .init(tier), usage: usage.map { CoxClient.Usage($0) },
        stop: stop.map { CoxClient.StopReason($0) })
    }
  }
}

extension CoxClient.TimelinePatch {
  init(_ value: CoxFFIBindings.TimelinePatch) {
    switch value {
    case .reset(let blocks): self = .reset(blocks: blocks.map { CoxClient.Block($0) })
    case .upsert(let block, let after): self = .upsert(block: .init(block), after: after)
    case .appendText(let id, let text): self = .appendText(id: id, text: text)
    case .docTail(let id, let from, let blocks):
      self = .docTail(id: id, from: from, blocks: blocks.map { CoxClient.DocBlock($0) })
    case .remove(let id): self = .remove(id: id)
    case .usage(let usage): self = .usage(usage: .init(usage))
    case .status(let status): self = .status(status: .init(status))
    case .pluginSlot(let slot): self = .pluginSlot(slot: .init(slot))
    }
  }
}

extension CoxClient.StyledDoc {
  init(_ value: CoxFFIBindings.StyledDoc) {
    self.init(blocks: value.blocks.map { CoxClient.DocBlock($0) })
  }
}

extension CoxClient.DocBlock {
  init(_ value: CoxFFIBindings.DocBlock) {
    let spans = { (lines: [[CoxFFIBindings.Span]]) in lines.map { $0.map { CoxClient.Span($0) } } }
    switch value {
    case .text(let kind, let lines):
      self = .text(
        kind: .init(kind),
        lines: lines.map {
          .init(
            $0.spans.map { CoxClient.Span($0) }, quote: $0.quote, depth: $0.depth,
            marker: $0.marker)
        })
    case .code(let lang, let lines): self = .code(lang: lang, lines: spans(lines))
    case .table(let rows): self = .table(rows: rows)
    case .rule: self = .rule
    }
  }
}

extension CoxClient.DiffModel {
  init(_ value: CoxFFIBindings.DiffModel) {
    self.init(
      path: value.path,
      hunks: value.hunks.map { hunk in
        .init(
          header: hunk.header, lines: hunk.lines.map { CoxClient.DiffLine($0) }, index: hunk.index)
      }, digest: value.digest)
  }
}

extension CoxClient.DiffLine {
  init(_ value: CoxFFIBindings.DiffLine) {
    let kind: CoxClient.DiffLineKind =
      switch value.kind {
      case .context: .context
      case .add: .add
      case .del: .del
      }
    self.init(
      kind: kind, old: value.old, new: value.new, spans: value.spans.map { CoxClient.Span($0) },
      words: value.words.map { .init(start: $0.start, end: $0.end) })
  }
}

extension CoxClient.TextKind {
  init(_ value: CoxFFIBindings.TextKind) {
    switch value {
    case .paragraph: self = .paragraph
    case .heading(let level): self = .heading(level)
    case .list: self = .list
    case .quote: self = .quote
    }
  }
}

extension CoxClient.Span {
  init(_ value: CoxFFIBindings.Span) {
    self.init(text: value.text)
    (token, rgb, light, link) = (.init(value.token), value.rgb, value.light, value.link)
    (bold, italic, strike, underline) = (value.bold, value.italic, value.strike, value.underline)
  }
}

extension CoxClient.StyleToken {
  init(_ value: CoxFFIBindings.StyleToken) {
    switch value {
    case .text: self = .text
    case .dim: self = .dim
    case .accent: self = .accent
    case .user: self = .user
    case .agent: self = .agent
    case .tool: self = .tool
    case .ok: self = .ok
    case .warn: self = .warn
    case .error: self = .error
    case .diffAdd: self = .diffAdd
    case .diffDel: self = .diffDel
    case .diffHunk: self = .diffHunk
    case .border: self = .border
    case .selection: self = .selection
    }
  }
}

extension CoxClient.Icon {
  init(_ value: CoxFFIBindings.Icon) {
    switch value {
    case .read: self = .read
    case .edit: self = .edit
    case .shell: self = .shell
    case .search: self = .search
    case .web: self = .web
    case .todo: self = .todo
    case .ask: self = .ask
    case .agent: self = .agent
    case .mcp: self = .mcp
    case .tool: self = .tool
    }
  }
}

extension CoxClient.Risk {
  init(_ value: CoxFFIBindings.Risk) {
    switch value {
    case .readOnly: self = .readOnly
    case .write: self = .write
    case .exec: self = .exec
    case .destructive: self = .destructive
    }
  }
}

extension CoxClient.ToolState {
  init(_ value: CoxFFIBindings.ToolState) {
    switch value {
    case .running: self = .running
    case .done: self = .done
    case .failed: self = .failed
    }
  }
}

extension CoxClient.Tier {
  init(_ value: CoxFFIBindings.Tier) {
    switch value {
    case .cheap: self = .cheap
    case .code: self = .code
    case .think: self = .think
    }
  }
}

extension CoxFFIBindings.Tier {
  init(_ value: CoxClient.Tier) {
    switch value {
    case .cheap: self = .cheap
    case .code: self = .code
    case .think: self = .think
    }
  }
}

extension CoxClient.Level {
  init(_ value: CoxFFIBindings.Level) {
    switch value {
    case .info: self = .info
    case .warn: self = .warn
    case .budget: self = .budget
    case .security: self = .security
    }
  }
}

extension CoxClient.DecidedBy {
  init(_ value: CoxFFIBindings.DecidedBy) {
    switch value {
    case .user: self = .user
    case .rule: self = .rule
    case .session: self = .session
    case .policy: self = .policy
    case .hook: self = .hook
    }
  }
}

extension CoxClient.CompactReason {
  init(_ value: CoxFFIBindings.CompactReason) {
    switch value {
    case .preCall: self = .preCall
    case .postTurn: self = .postTurn
    case .manual: self = .manual
    case .contextTooLong: self = .contextTooLong
    }
  }
}

extension CoxClient.Why {
  init(_ value: CoxFFIBindings.Why) {
    switch value {
    case .ruleAsk(let rule): self = .ruleAsk(rule: rule)
    case .risk(let risk): self = .risk(risk: .init(risk))
    case .sandboxDenied(let detail): self = .sandboxDenied(detail: detail)
    case .policy(let policy):
      switch policy {
      case .untrusted: self = .policy(policy: .untrusted)
      case .onRequest: self = .policy(policy: .onRequest)
      case .onFailure: self = .policy(policy: .onFailure)
      case .never: self = .policy(policy: .never)
      }
    }
  }
}

extension CoxClient.Decision {
  init(_ value: CoxFFIBindings.Decision) {
    switch value {
    case .allow: self = .allow
    case .allowForSession: self = .allowForSession
    case .deny(let reason): self = .deny(reason: reason)
    case .edit(let input): self = .edit(input: input)
    }
  }
}

extension CoxFFIBindings.Decision {
  init(_ value: CoxClient.Decision) {
    switch value {
    case .allow: self = .allow
    case .allowForSession: self = .allowForSession
    case .deny(let reason): self = .deny(reason: reason)
    case .edit(let input): self = .edit(input: input)
    }
  }
}

extension CoxClient.StopReason {
  init(_ value: CoxFFIBindings.StopReason) {
    switch value {
    case .endTurn: self = .endTurn
    case .maxTurns: self = .maxTurns
    case .interrupted: self = .interrupted
    case .budget: self = .budget
    case .refusal(let detail): self = .refusal(detail: detail)
    case .error: self = .error
    }
  }
}

extension CoxClient.Usage {
  init(_ value: CoxFFIBindings.Usage) {
    self.init(
      inputTokens: value.inputTokens, outputTokens: value.outputTokens,
      cacheReadTokens: value.cacheReadTokens, cacheWriteTokens: value.cacheWriteTokens,
      estimated: value.estimated, costUsd: value.costUsd, latencyMs: value.latencyMs)
  }
}

extension CoxClient.UsageView {
  init(_ value: CoxFFIBindings.UsageView) {
    self.init(
      session: .init(value.session), turn: value.turn.map { CoxClient.TurnUsage($0) },
      contextTokens: value.contextTokens, text: .init(value.text))
  }
}

extension CoxClient.Tally {
  init(_ value: CoxFFIBindings.Tally) {
    self.init(
      sent: value.sent, received: value.received, cacheRead: value.cacheRead,
      cacheWrite: value.cacheWrite,
      uncached: value.uncached, costUsd: value.costUsd, calls: value.calls,
      estimated: value.estimated)
  }
}

extension CoxClient.TurnUsage {
  init(_ value: CoxFFIBindings.TurnUsage) {
    self.init(
      turn: value.turn, tally: .init(value.tally), thinkingTokens: value.thinkingTokens,
      ttftMs: value.ttftMs,
      tokPerS: value.tokPerS, exact: value.exact, sparkline: value.sparkline, done: value.done)
  }
}

extension CoxFFIBindings.Intent {
  init(_ value: CoxClient.Intent) {
    switch value {
    case .send(let text, let files, let think):
      self = .send(text: text, attachments: files.map { .init($0) }, confirmThink: think)
    case .approve(let call, let decision): self = .approve(call: call, decision: .init(decision))
    case .answer(let question, let text): self = .answer(question: question, text: text)
    case .interrupt: self = .interrupt
    case .queue(let text, let files, let think):
      self = .queue(text: text, attachments: files.map { .init($0) }, confirmThink: think)
    case .compact(let focus): self = .compact(focus: focus)
    case .setMode(let mode):
      switch mode {
      case .default: self = .setMode(mode: .default)
      case .plan: self = .setMode(mode: .plan)
      case .auto: self = .setMode(mode: .auto)
      case .bypass: self = .setMode(mode: .bypass)
      }
    case .switchModel(let tier, let model): self = .switchModel(tier: .init(tier), model: model)
    case .setEffort(let effort):
      switch effort {
      case nil: self = .setEffort(effort: nil)
      case .low: self = .setEffort(effort: .low)
      case .medium: self = .setEffort(effort: .medium)
      case .high: self = .setEffort(effort: .high)
      case .xhigh: self = .setEffort(effort: .xhigh)
      }
    case .rewind(let toTurn, let code, let conversation):
      self = .rewind(toTurn: toTurn, code: code, conversation: conversation)
    case .redo: self = .redo
    case .revertFile(let path, let toTurn): self = .revertFile(path: path, toTurn: toTurn)
    case .revertHunk(let path, let toTurn, let hunk, let nowDigest):
      self = .revertHunk(path: path, toTurn: toTurn, hunk: hunk, nowDigest: nowDigest)
    case .fork(let turn): self = .fork(turn: turn)
    case .handoff(let objective): self = .handoff(objective: objective)
    case .background(let call): self = .background(call: call)
    case .shell(let command, let share): self = .shell(command: command, share: share)
    case .command(let line): self = .command(line: line)
    case .rename(let title): self = .rename(title: title)
    }
  }
}

extension CoxFFIBindings.Attachment {
  init(_ value: CoxClient.Attachment) {
    self.init(name: value.name, mediaType: value.mediaType, dataB64: value.dataB64)
  }
}

/// The core's Markdown writer (T58.4.27), for `DocMarkdown.writer`: the doc goes back over the
/// seam as the generated values it came from, so nothing the core's writer reads is dropped.
public struct CoreDocWriter: DocWriter {
  public init() {}

  public func markdown(_ doc: CoxClient.StyledDoc) -> String {
    CoxFFIBindings.docMarkdown(doc: .init(doc))
  }

  public func markdown(_ block: CoxClient.DocBlock) -> String? {
    CoxFFIBindings.blockMarkdown(block: .init(block))
  }
}

extension CoxFFIBindings.StyledDoc {
  init(_ value: CoxClient.StyledDoc) {
    self.init(blocks: value.blocks.map { CoxFFIBindings.DocBlock($0) })
  }
}

extension CoxFFIBindings.DocBlock {
  init(_ value: CoxClient.DocBlock) {
    let spans = { (lines: [[CoxClient.Span]]) in lines.map { $0.map { CoxFFIBindings.Span($0) } } }
    switch value {
    case .text(let kind, let lines):
      self = .text(
        kind: .init(kind),
        lines: lines.map {
          .init(
            quote: $0.quote, depth: $0.depth, marker: $0.marker,
            spans: $0.spans.map { CoxFFIBindings.Span($0) })
        })
    case .code(let lang, let lines): self = .code(lang: lang, lines: spans(lines))
    case .table(let rows): self = .table(rows: rows)
    case .rule: self = .rule
    }
  }
}

extension CoxFFIBindings.TextKind {
  init(_ value: CoxClient.TextKind) {
    switch value {
    case .paragraph: self = .paragraph
    case .heading(let level): self = .heading(level)
    case .list: self = .list
    case .quote: self = .quote
    }
  }
}

extension CoxFFIBindings.Span {
  init(_ value: CoxClient.Span) {
    self.init(
      text: value.text, token: .init(value.token), rgb: value.rgb, light: value.light,
      bold: value.bold, italic: value.italic, strike: value.strike, underline: value.underline,
      link: value.link)
  }
}

extension CoxFFIBindings.StyleToken {
  init(_ value: CoxClient.StyleToken) {
    switch value {
    case .text: self = .text
    case .dim: self = .dim
    case .accent: self = .accent
    case .user: self = .user
    case .agent: self = .agent
    case .tool: self = .tool
    case .ok: self = .ok
    case .warn: self = .warn
    case .error: self = .error
    case .diffAdd: self = .diffAdd
    case .diffDel: self = .diffDel
    case .diffHunk: self = .diffHunk
    case .border: self = .border
    case .selection: self = .selection
    }
  }
}
