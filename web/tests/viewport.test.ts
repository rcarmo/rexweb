import { describe, expect, test } from "bun:test";
import { inputFrame, visibleFrame, workspaceFrame } from "../src/viewport";

const visual = { offsetLeft: 0, offsetTop: 0, width: 1024, height: 768, scale: 1 };

describe("Safari keyboard viewport geometry", () => {
  test("follows a floating input indicator's pan without losing the workspace top", () => {
    expect(workspaceFrame(1024, 768, { ...visual, offsetTop: 64, height: 704 }))
      .toEqual({ left: 0, top: 64, width: 1024, height: 704 });
  });
  test("uses the reduced visible area for a docked keyboard", () => {
    expect(workspaceFrame(1024, 768, { ...visual, height: 420 }).height).toBe(420);
  });
  test("does not resize/reposition the workspace during user pinch zoom", () => {
    const zoom = { ...visual, scale: 2, width: 512, height: 384, offsetTop: 100 };
    expect(workspaceFrame(1024, 768, zoom)).toEqual({ left: 0, top: 0, width: 1024, height: 768 });
    expect(visibleFrame(1024, 768, zoom).top).toBe(100);
  });
  test("falls back safely without VisualViewport or with transient invalid metrics", () => {
    expect(workspaceFrame(1024, 768)).toEqual({ left: 0, top: 0, width: 1024, height: 768 });
    expect(workspaceFrame(1024, 768, { ...visual, height: 0 }).height).toBe(768);
    expect(visibleFrame(1024, 768, { ...visual, offsetTop: NaN }).top).toBe(0);
  });
  test("fits the entire caret box above the visible bottom, not just its top pixel", () => {
    const viewport = { left: 0, top: 80, width: 1024, height: 420 };
    const result = inputFrame({ left: 1400, top: 740, height: 20 }, viewport)!;
    expect(result.left).toBe(1023);
    expect(result.height).toBe(20);
    expect(result.top + result.height).toBe(484);
    expect(result.top).toBeGreaterThanOrEqual(viewport.top);
  });
  test("keeps ordinary cursor anchoring and a 16px native caret", () => {
    const viewport = { left: 0, top: 0, width: 1024, height: 768 };
    expect(inputFrame({ left: 150, top: 240, height: 20 }, viewport))
      .toEqual({ left: 150, top: 240, width: 1, height: 20 });
    expect(inputFrame({ left: 150, top: 240, height: 8 }, viewport)?.height).toBe(16);
  });
  test("handles tiny viewports and invalid anchors without out-of-bounds coordinates", () => {
    expect(inputFrame({ left: -50, top: -50, height: 20 }, { left: 0, top: 0, width: 1, height: 8 }))
      .toEqual({ left: 0, top: 0, width: 1, height: 8 });
    expect(inputFrame({ left: NaN, top: 0, height: 20 }, { left: 0, top: 0, width: 100, height: 100 })).toBeUndefined();
  });
});
