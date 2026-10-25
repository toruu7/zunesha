import { describe, expect, it } from "vitest";

import { buildGraph, layoutDag, rankTasks } from "./dag";
import type { DependencyEdge } from "./types";

/** `task_id` depends on `depends_on`. */
function edge(taskId: string, dependsOn: string): DependencyEdge {
  return { task_id: taskId, depends_on: dependsOn };
}

// The planner emits a 3-task chain (head ← t2 ← t3) — the common case.
const chain = ["head", "t2", "t3"];
const chainDeps = [edge("t2", "head"), edge("t3", "t2")];

describe("rankTasks", () => {
  it("ranks a chain by dependency depth", () => {
    const rank = rankTasks(chain, chainDeps);
    expect(rank.get("head")).toBe(0);
    expect(rank.get("t2")).toBe(1);
    expect(rank.get("t3")).toBe(2);
  });

  it("uses the *longest* path when a task joins branches of different depth", () => {
    //     a           e
    //    / \          |
    //   b   c         f
    //    \ /          |
    //     d <--- g <--+   (d depends on b, c, *and* g; g's branch is deeper)
    const ids = ["a", "b", "c", "d", "e", "f", "g"];
    const deps = [
      edge("b", "a"),
      edge("c", "a"),
      edge("d", "b"),
      edge("d", "c"),
      edge("f", "e"),
      edge("g", "f"),
      edge("d", "g"),
    ];
    const rank = rankTasks(ids, deps);
    expect(rank.get("b")).toBe(1);
    expect(rank.get("g")).toBe(2);
    expect(rank.get("d"), "the deepest branch sets the column").toBe(3);
  });

  it("leaves independent tasks at rank 0", () => {
    const rank = rankTasks(["x", "y"], []);
    expect(rank.get("x")).toBe(0);
    expect(rank.get("y")).toBe(0);
  });

  it("ignores edges pointing at unknown tasks (no phantom columns)", () => {
    const rank = rankTasks(["t2"], [edge("t2", "deleted-head")]);
    expect(rank.get("t2")).toBe(0);
  });

  it("terminates on a corrupt cycle instead of hanging the view", () => {
    // The planner can't emit this, but a corrupt DB must not freeze the UI:
    // the pass cap bounds the work (exact ranks are meaningless in a cycle).
    const rank = rankTasks(["a", "b"], [edge("a", "b"), edge("b", "a")]);
    expect(rank.size, "every task still gets a rank").toBe(2);
    expect(rank.get("a")).toBeGreaterThanOrEqual(1);
    expect(rank.get("b")).toBeGreaterThanOrEqual(1);
  });
});

describe("layoutDag", () => {
  it("places ranks left-to-right and stacks a rank top-to-bottom", () => {
    const nodes = layoutDag(chain, chainDeps);
    expect(nodes).toEqual([
      { id: "head", rank: 0, x: 0, y: 0 },
      { id: "t2", rank: 1, x: 260, y: 0 },
      { id: "t3", rank: 2, x: 520, y: 0 },
    ]);
  });

  it("stacks same-rank tasks in creation order", () => {
    const nodes = layoutDag(["head", "b", "c"], [edge("b", "head"), edge("c", "head")]);
    expect(nodes).toEqual([
      { id: "head", rank: 0, x: 0, y: 0 },
      { id: "b", rank: 1, x: 260, y: 0 },
      { id: "c", rank: 1, x: 260, y: 110 },
    ]);
  });

  it("lays out nothing for an empty plan", () => {
    expect(layoutDag([], [])).toEqual([]);
  });
});

describe("buildGraph", () => {
  it("emits one edge per dependency, pointing prerequisite → dependent", () => {
    const graph = buildGraph(chain, chainDeps);
    expect(graph.edges).toEqual([
      { id: "head->t2", source: "head", target: "t2" },
      { id: "t2->t3", source: "t2", target: "t3" },
    ]);
    expect(graph.nodes).toHaveLength(3);
  });

  it("drops edges whose endpoints are missing", () => {
    const graph = buildGraph(["t2"], chainDeps);
    expect(graph.edges).toEqual([]);
  });

  it("is idempotent: the same inputs give the same edge ids (reactflow keys)", () => {
    const once = buildGraph(chain, chainDeps);
    const twice = buildGraph(chain, chainDeps);
    expect(twice.edges).toEqual(once.edges);
  });
});
