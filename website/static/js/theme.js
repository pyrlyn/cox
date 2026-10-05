// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

/*! cox theme toggle — cycles system → light → dark.
   Boot snippet in baseof.html applies the theme before paint to avoid FOUC.
   Persist key: localStorage["cox-theme"] = "system" | "light" | "dark"
*/
(function () {
  const KEY = "cox-theme";
  const ORDER = ["system", "light", "dark"];

  function stored() {
    try {
      return localStorage.getItem(KEY);
    } catch {
      return null;
    }
  }

  function resolve(preference) {
    if (preference === "light" || preference === "dark") return preference;
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  }

  function apply(preference) {
    const pref = ORDER.includes(preference) ? preference : "system";
    const resolved = resolve(pref);
    const root = document.documentElement;
    root.setAttribute("data-theme", pref);
    root.setAttribute("data-theme-resolved", resolved);
    root.style.colorScheme = resolved;
    syncToggle(pref, resolved);
  }

  function syncToggle(pref, resolved) {
    const btn = document.getElementById("theme-toggle");
    if (!btn) return;
    const labels = {
      system: "Theme: system (follows OS)",
      light: "Theme: light",
      dark: "Theme: dark",
    };
    btn.setAttribute("aria-label", labels[pref] || labels.system);
    btn.dataset.theme = pref;
    btn.dataset.resolved = resolved;
    const label = btn.querySelector("[data-theme-label]");
    if (label) label.textContent = pref;
  }

  function cycle() {
    const current = stored() || "system";
    const idx = ORDER.indexOf(current);
    const next = ORDER[(idx + 1) % ORDER.length];
    try {
      localStorage.setItem(KEY, next);
    } catch {
      /* private mode */
    }
    apply(next);
  }

  window.__coxTheme = { apply, cycle, resolve, KEY };

  document.addEventListener("DOMContentLoaded", () => {
    apply(stored() || "system");
    const btn = document.getElementById("theme-toggle");
    if (btn) btn.addEventListener("click", cycle);
  });

  try {
    window
      .matchMedia("(prefers-color-scheme: dark)")
      .addEventListener("change", () => {
        const pref = stored() || "system";
        if (pref === "system") apply("system");
      });
  } catch {
    /* older Safari */
  }
})();
