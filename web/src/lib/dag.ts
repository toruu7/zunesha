// Pure layout math for the interactive task-DAG view (Phase 11).
//
// No React and no reactflow types here: the layering is plain data that
// `dag-view.tsx` turns into reactflow nodes/edges, so the placement rules are
// unit-testable without a DOM.
//
// Layout is a layered ("rank") layout: a task's column is its dependency
// depth — roots at x = 0, everything they unblock one column to the right —
// and within a column tasks keep their creation order. That reads exactly
// like the pipeline the engine enforces: work flows left to right as
// prerequisites turn DONE. No layout library is bundled for this; the
// planner only ever emits a small chain/DAG, where longest-path layering is
// a few lines and stays put while statuses change live.

import type { DependencyEdge } from "./types";

/** Horizontal gap between adjacent ranks (columns). */
const RANK_GAP = 260;

/** Vertical gap between adjacent nodes inside one rank. */
const ROW_GAP = 110;

/** A positioned node: a task placed at a computed graph coordinate. */
export interface DagNode {
  id: string;
  /** Left edge of the node, in graph units. */
  x: number;
  /** Top edge of the node, in graph units. */
  y: number;
  /** Dependency depth: 0 for tasks that nothing blocks. */
  rank: number;
}

/** One rendered edge: `source` (prerequisite) points at `target`. */
export interface DagEdge {
  id: string;
  source: string;
  target: string;
}

/** The graph the view renders: positioned nodes plus their edges. */
export interface DagGraph {
  nodes: DagNode[];
  edges: DagEdge[];
}

/** Keep only edges whose endpoints are actual tasks (stale edges are dropped). */
function knownEdges(
  taskIds: Set<string>,
  deps: readonly DependencyEdge[],
): DependencyEdge[] {
  return deps.filter((e) => taskIds.has(e.task_id) && taskIds.has(e.depends_on));
}

/**
 * Assign every task its dependency depth: the longest path to any root.
 *
 * Computed by iterative relaxation ("rank(t) = max over deps, +1") until
 * stable, which converges on a DAG in at most N passes. The planner only
 * creates edges topologically so cycles can't reach the database, but a
 * corrupt one must not hang the UI either: the pass cap bounds the work and
 * leaves whatever ranks the relaxation settled at.
 *
 * Edges pointing at unknown task ids are ignored, so a stale edge can't
 * create a phantom column.
 */
export function rankTasks(
  taskIds: readonly string[],
  deps: readonly DependencyEdge[],
): Map<string, number> {
  const known = new Set(taskIds);
  const edges = knownEdges(known, deps);
  const rank = new Map<string, number>(taskIds.map((id) => [id, 0]));

  for (let pass = 0; pass < taskIds.length; pass += 1) {
    let changed = false;
    for (const { task_id: taskId, depends_on: dependsOn } of edges) {
      const current = rank.get(taskId) ?? 0;
      const needed = (rank.get(dependsOn) ?? 0) + 1;
      if (needed > current) {
        rank.set(taskId, needed);
        changed = true;
      }
    }
    if (!changed) break; // stable
  }
  return rank;
}

/**
 * Place every task on the layered grid.
 *
 * `taskIds` is the task order (creation order from the API); nodes stack
 * top-to-bottom within their rank in that same order, so a re-plan or a
 * status change never reshuffles unrelated nodes.
 */
export function layoutDag(
  taskIds: readonly string[],
  deps: readonly DependencyEdge[],
): DagNode[] {
  const rank = rankTasks(taskIds, deps);
  const rowsInRank = new Map<number, number>();

  return taskIds.map((id) => {
    const r = rank.get(id) ?? 0;
    const row = rowsInRank.get(r) ?? 0;
    rowsInRank.set(r, row + 1);
    return { id, rank: r, x: r * RANK_GAP, y: row * ROW_GAP };
  });
}

/**
 * Build the full graph the DAG view renders.
 *
 * Edges point from prerequisite to dependent (arrows flow with the
 * pipeline). Ids are derived from the endpoints, so the edge set is
 * idempotent across re-renders — reactflow keys on them.
 */
export function buildGraph(
  taskIds: readonly string[],
  deps: readonly DependencyEdge[],
): DagGraph {
  const known = new Set(taskIds);
  const edges = knownEdges(known, deps).map(
    ({ task_id: taskId, depends_on: dependsOn }) => ({
      id: `${dependsOn}->${taskId}`,
      source: dependsOn,
      target: taskId,
    }),
  );
  return { nodes: layoutDag(taskIds, deps), edges };
}
