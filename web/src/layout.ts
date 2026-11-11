export type Direction = "horizontal" | "vertical";
export type Rect = [number, number, number, number];
export type Layout = {
  rect: Rect;
  block_id?: string | null;
  split?: { split_id: string; direction: Direction; ratio: number; before: Layout; after: Layout } | null;
};

/** Project a temporary split ratio into pane rectangles without changing server state. */
export function previewSplit(node: Layout, ratio: number): Layout {
  function reflow(current: Layout, rect: Rect, root = false): Layout {
    if (!current.split) return { ...current, rect };
    const split = current.split;
    const nextRatio = root ? Math.min(.95, Math.max(.05, ratio)) : split.ratio;
    const [x, y, width, height] = rect;
    const horizontal = split.direction === "horizontal";
    const before: Rect = horizontal ? [x, y, width * nextRatio, height] : [x, y, width, height * nextRatio];
    const after: Rect = horizontal ? [x + width * nextRatio, y, width * (1 - nextRatio), height] :
      [x, y + height * nextRatio, width, height * (1 - nextRatio)];
    return { ...current, rect, split: { ...split, ratio: nextRatio,
      before: reflow(split.before, before), after: reflow(split.after, after) } };
  }
  return reflow(node, node.rect, true);
}
