import type { StackEntry } from '../../../shared/stack.ts';

// A git-graph row for one stack entry: which lane its dot sits in, and which
// lines cross this row so the SVG cell can be drawn without knowing about
// any other row.
export interface GraphRow<T extends StackEntry = StackEntry> {
  entry: T;
  lane: number;
  parentLane: number | null;
  through: number[];
  mergeFrom: number[];
  hasChildInLane: boolean;
  hasParentBelow: boolean;
}

// Lays out a stack as a `git log --graph`-style tree: newest (leaves) at the
// top, trunk at the bottom, with a new lane for every fork. `entries` must
// list parents before children (the shape the stack API already returns).
// Generic so callers with a richer entry type (e.g. the inbox's per-owner
// stacks) get it back on each row instead of the base `StackEntry`.
export function layoutStackGraph<T extends StackEntry>(
  entries: T[],
): GraphRow<T>[] {
  if (entries.length === 0) return [];

  const byNumber = new Map(entries.map((entry) => [entry.number, entry]));
  const childrenOf = new Map<number, T[]>();
  const roots: T[] = [];
  for (const entry of entries) {
    const parent =
      entry.parent != null ? byNumber.get(entry.parent) : undefined;
    if (parent) {
      const siblings = childrenOf.get(parent.number) ?? [];
      siblings.push(entry);
      childrenOf.set(parent.number, siblings);
    } else {
      roots.push(entry);
    }
  }
  for (const siblings of childrenOf.values()) {
    siblings.sort((a, b) => a.number - b.number);
  }
  roots.sort((a, b) => a.number - b.number);

  const lanes = new Map<number, number>();
  const preorder: T[] = [];
  let nextLane = 0;

  function walk(entry: T, lane: number): void {
    lanes.set(entry.number, lane);
    preorder.push(entry);
    const children = childrenOf.get(entry.number) ?? [];
    children.forEach((child, i) => {
      if (i === 0) {
        walk(child, lane);
      } else {
        nextLane += 1;
        walk(child, nextLane);
      }
    });
  }

  roots.forEach((root, i) => {
    if (i === 0) {
      walk(root, nextLane);
    } else {
      nextLane += 1;
      walk(root, nextLane);
    }
  });

  // Reversing the pre-order puts the first child (and its whole subtree)
  // directly above its parent, with later siblings' subtrees above that —
  // newest work at the top.
  const topFirst = [...preorder].reverse();
  const indexOf = new Map(topFirst.map((entry, i) => [entry.number, i]));

  return topFirst.map((entry, i) => {
    const lane = lanes.get(entry.number) ?? 0;
    const parentEntry =
      entry.parent != null ? byNumber.get(entry.parent) : undefined;
    const parentLane = parentEntry
      ? (lanes.get(parentEntry.number) ?? 0)
      : null;

    // A lane passes straight through this row when some node above it has
    // not yet reached its parent's row.
    const through: number[] = [];
    for (const other of topFirst) {
      if (other.number === entry.number) continue;
      const otherIndex = indexOf.get(other.number);
      if (otherIndex == null || otherIndex >= i) continue;
      const otherParent =
        other.parent != null ? byNumber.get(other.parent) : undefined;
      if (!otherParent) continue;
      const otherParentIndex = indexOf.get(otherParent.number);
      if (otherParentIndex != null && otherParentIndex > i) {
        const otherLane = lanes.get(other.number) ?? 0;
        if (!through.includes(otherLane)) through.push(otherLane);
      }
    }

    const children = childrenOf.get(entry.number) ?? [];
    const mergeFrom: number[] = [];
    for (const child of children) {
      const childLane = lanes.get(child.number) ?? 0;
      if (childLane !== lane && !mergeFrom.includes(childLane)) {
        mergeFrom.push(childLane);
      }
    }
    const hasChildInLane = children.some(
      (child) => (lanes.get(child.number) ?? 0) === lane,
    );

    return {
      entry,
      lane,
      parentLane,
      through,
      mergeFrom,
      hasChildInLane,
      hasParentBelow: parentEntry != null,
    };
  });
}
