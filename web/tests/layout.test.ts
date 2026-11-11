import { describe, expect, test } from "bun:test";
import { previewSplit, type Layout } from "../src/layout";

const layout: Layout = {
  rect: [.2, .1, .8, .9],
  split: {
    split_id: "columns", direction: "horizontal", ratio: .5,
    before: { block_id: "left", rect: [.2, .1, .4, .9] },
    after: {
      rect: [.6, .1, .4, .9],
      split: {
        split_id: "rows", direction: "vertical", ratio: .4,
        before: { block_id: "top-right", rect: [.6, .1, .4, .36] },
        after: { block_id: "bottom-right", rect: [.6, .46, .4, .54] },
      },
    },
  },
};

function expectRect(actual: number[], expected: number[]) {
  actual.forEach((value, index) => expect(value).toBeCloseTo(expected[index]));
}

describe("live split preview", () => {
  test("resizes a nested layout within its parent without changing the daemon view", () => {
    const original = structuredClone(layout);
    const preview = previewSplit(layout, .75);
    expectRect(preview.split!.before.rect, [.2, .1, .6, .9]);
    expectRect(preview.split!.after.rect, [.8, .1, .2, .9]);
    expectRect(preview.split!.after.split!.before.rect, [.8, .1, .2, .36]);
    expectRect(preview.split!.after.split!.after.rect, [.8, .46, .2, .54]);
    expect(preview.split!.after.split!.ratio).toBe(.4);
    expect(layout).toEqual(original);
  });

  test("clamps vertical drags at the same limits as the committed resize", () => {
    const rows = layout.split!.after;
    const low = previewSplit(rows, -1);
    const high = previewSplit(rows, 2);
    expect(low.split!.ratio).toBe(.05);
    expect(high.split!.ratio).toBe(.95);
    expectRect(low.split!.before.rect, [.6, .1, .4, .045]);
    expectRect(high.split!.after.rect, [.6, .955, .4, .045]);
  });
});
