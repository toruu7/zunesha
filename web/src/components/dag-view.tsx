"use client";

import { useEffect, useMemo } from "react";
import {
  Background,
  Controls,
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  useEdgesState,
  useNodesState,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";

import { buildGraph } from "@/lib/dag";
import { ACCENT, isActive } from "@/lib/status";
import { useZou } from "@/lib/store";

/**
 * One task on the canvas: the same card the kanban board shows, minus the
 * column it happens to sit in.
 *
 * It reads its task straight from the store, so a live status change
 * repaints the node without the graph (positions, edges) ever being
 * rebuilt — the view doesn't jump around or reset pan/zoom while agents
 * stream.
 */
function TaskNode({ id }: NodeProps) {
  const task = useZou((s) => s.tasks.find((t) => t.id === id));
  const select = useZou((s) => s.select);
  if (!task) return null;

  return (
    <button
      onClick={() => select(task.id)}
      className="group w-56 text-left rounded-md border border-zinc-800 bg-zinc-900/90 hover:border-zinc-600 hover:bg-zinc-800/80 transition-colors overflow-hidden cursor-pointer"
    >
      <Handle type="target" position={Position.Left} />
      <div className="flex">
        <div className={`w-1 shrink-0 ${ACCENT[task.status]}`} />
        <div className="p-2 min-w-0">
          <div className="flex items-center gap-1.5">
            <code className="text-[10px] text-zinc-500 font-mono shrink-0">
              {task.id}
            </code>
            {isActive(task.status) && (
              <span className="inline-block size-1.5 rounded-full bg-amber-400 animate-pulse shrink-0" />
            )}
          </div>
          <p className="text-xs text-zinc-100 leading-snug mt-0.5 line-clamp-2 break-words">
            {task.title}
          </p>
        </div>
      </div>
      <Handle type="source" position={Position.Right} />
    </button>
  );
}

const NODE_TYPES = { task: TaskNode };

/**
 * The interactive task-DAG view (Phase 11).
 *
 * Nodes are tasks, edges are the plan's dependency list drawn prerequisite →
 * dependent, so the shape mirrors the pipeline the engine enforces: work
 * flows left to right as prerequisites turn DONE. Positions come from the
 * pure layered layout in `@/lib/dag` — no layout library — and clicking a
 * node opens the same review/detail modal the board uses.
 */
export default function DagView() {
  const tasks = useZou((s) => s.tasks);
  const deps = useZou((s) => s.deps);
  const select = useZou((s) => s.select);

  // Rebuild only when the task *set* or the plan changes. Task objects change
  // identity on every live update, so the memo key is the joined ids — a
  // status change must not reset the user's pan/zoom or dragged nodes.
  const idsKey = tasks.map((t) => t.id).join(",");
  const graph = useMemo(
    () => buildGraph(idsKey === "" ? [] : idsKey.split(","), deps),
    [idsKey, deps],
  );

  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);

  useEffect(() => {
    setNodes(
      graph.nodes.map((n) => ({
        id: n.id,
        type: "task",
        position: { x: n.x, y: n.y },
        data: {},
      })),
    );
    setEdges(
      graph.edges.map((e) => ({
        id: e.id,
        source: e.source,
        target: e.target,
        markerEnd: { type: MarkerType.ArrowClosed },
      })),
    );
  }, [graph, setNodes, setEdges]);

  if (tasks.length === 0) {
    return (
      <div className="flex-1 min-h-0 flex items-center justify-center rounded-lg border border-dashed border-zinc-800">
        <p className="text-sm text-zinc-600">
          no tasks yet — plan a feature to draw its graph
        </p>
      </div>
    );
  }

  return (
    <div className="flex-1 min-h-0 rounded-lg border border-zinc-800 overflow-hidden">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onNodeClick={(_, node) => select(node.id)}
        fitView
        minZoom={0.2}
        colorMode="dark"
        // The DAG is the engine's, not the user's to rewire: pan, zoom, and
        // drag are fine, but nothing here can create or delete edges.
        nodesConnectable={false}
        deleteKeyCode={null}
      >
        <Background gap={18} color="#27272a" />
        <Controls showInteractive={false} />
      </ReactFlow>
    </div>
  );
}
