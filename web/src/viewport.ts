/** Coordinates in CSS pixels relative to the layout viewport. */
export interface ViewportFrame {
  left: number;
  top: number;
  width: number;
  height: number;
}

type VisualFrame = Pick<VisualViewport, "offsetLeft" | "offsetTop" | "width" | "height" | "scale">;

function validFrame(frame: ViewportFrame): boolean {
  return Object.values(frame).every(Number.isFinite) && frame.width > 0 && frame.height > 0;
}

export function visibleFrame(width: number, height: number, visual?: VisualFrame | null): ViewportFrame {
  const frame = visual && { left: visual.offsetLeft, top: visual.offsetTop, width: visual.width, height: visual.height };
  return frame && validFrame(frame) ? frame : { left: 0, top: 0, width, height };
}

/** Follow keyboard panning/resizing at normal scale, not a user's pinch zoom. */
export function workspaceFrame(width: number, height: number, visual?: VisualFrame | null): ViewportFrame {
  return visual && Math.abs(visual.scale - 1) < 0.01
    ? visibleFrame(width, height, visual)
    : { left: 0, top: 0, width, height };
}

/** Keep the entire native caret box inside the viewport, with breathing room. */
export function inputFrame(anchor: { left: number; top: number; height: number }, viewport: ViewportFrame): ViewportFrame | undefined {
  if (!validFrame(viewport) || !Object.values(anchor).every(Number.isFinite)) return;
  const height = Math.min(viewport.height, Math.max(16, anchor.height));
  const inset = Math.min(16, Math.max(0, (viewport.height - height) / 2));
  return {
    left: Math.max(viewport.left, Math.min(viewport.left + viewport.width - 1, anchor.left)),
    top: Math.max(viewport.top + inset, Math.min(viewport.top + viewport.height - height - inset, anchor.top)),
    width: 1,
    height,
  };
}

/** One workspace-level observer; terminal panes keep their own internal scrolling. */
export function bindWorkspaceViewport(): () => void {
  const root = document.documentElement;
  const viewport = window.visualViewport;
  let frame: number | undefined;
  const update = () => {
    frame = undefined;
    const bounds = workspaceFrame(window.innerWidth, window.innerHeight, viewport);
    if (!validFrame(bounds)) return;
    for (const [name, value] of Object.entries(bounds)) root.style.setProperty(`--workspace-${name}`, `${value}px`);
  };
  const schedule = () => { frame ??= window.requestAnimationFrame(update); };
  window.addEventListener("resize", schedule);
  viewport?.addEventListener("resize", schedule);
  viewport?.addEventListener("scroll", schedule);
  update();
  return () => {
    if (frame !== undefined) window.cancelAnimationFrame(frame);
    window.removeEventListener("resize", schedule);
    viewport?.removeEventListener("resize", schedule);
    viewport?.removeEventListener("scroll", schedule);
    for (const name of ["left", "top", "width", "height"]) root.style.removeProperty(`--workspace-${name}`);
  };
}
