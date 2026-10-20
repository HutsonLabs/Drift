// Statistics HUD (plan M1 "Done (manual M1)", M9-1): a small non-interactive panel that floats
// in the corner of the live picture. Rust shrinks the web view to that corner
// (`present::hud_frame`), so everything outside this panel still reaches the remote desktop.
// Rust quantises every number to tenths of its displayed unit (`StatsView`), so the panel is
// only redrawn when the line really changes.
import type { StatsView } from "../bindings";
import { h, mount } from "../dom";

/** Tenths of a unit as a one-decimal string ("589" → "58.9"). */
function decimal(tenths: number): string {
  return (tenths / 10).toFixed(1);
}

/** The readouts, frame rate first — the number the M1 acceptance step reads. */
function readouts(stats: StatsView): [string, string][] {
  return [
    [decimal(stats.fps_tenths), "fps"],
    [decimal(stats.mbit_tenths), "Mbit/s"],
    [decimal(stats.latency_p95_tenths_ms), "ms"],
    [String(stats.unacked_frames), "unacked"],
  ];
}

/** The one-line summary ("58.9 fps · 1.2 Mbit/s · 4.3 ms · 2 unacked"). */
export function statsLine(stats: StatsView): string {
  return readouts(stats)
    .map(([v, u]) => `${v} ${u}`)
    .join(" · ");
}

/** Renders the HUD. It has no controls: the picture underneath keeps the keyboard and mouse. */
export function renderStatsHud(root: HTMLElement, stats: StatsView): void {
  mount(
    root,
    h(
      "aside",
      { class: "hud stats", role: "status", "aria-live": "polite", "aria-label": "Session statistics" },
      readouts(stats).map(([v, u]) => h("span", { class: "readout" }, h("span", { class: "value" }, v), " ", h("span", { class: "unit" }, u))),
    ),
  );
}
