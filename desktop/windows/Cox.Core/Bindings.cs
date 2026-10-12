// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later

namespace Cox.Core;

/// <summary>
/// Home for the generated C# bindings. The generator is the local fork of
/// uniffi-bindgen-cs at 53fe29f: PR #176 (uniffi 0.32) plus PR #166 (callback
/// interfaces return <c>Task</c>). Upstream has not merged either. Until the
/// bindings are generated this project only references <c>Cox.Model</c>,
/// matching CoxCore's dependency on CoxClient (DT§4.6).
/// </summary>
public static class Bindings
{
}
