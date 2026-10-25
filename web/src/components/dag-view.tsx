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
      className="group w-56 text-left border-4 border-ink bg-contrast shadow-brutal-sm hover:-translate-x-0.5 hover:-translate-y-0.5 hover:shadow-brutal active:translate-x-0 active:translate-y-0 active:shadow-brutal-sm focus-visible:outline-4 cursor-pointer"
    >
      <Handle type="target" position={Position.Left} />
      <div className="flex">
        <div className={`w-2 shrink-0 border-r-[3px] border-ink ${ACCENT[task.status]}`} />
        <div className="p-2 min-w-0">
          <div className="flex items-center gap-1.5">
            <code className="shrink-0 border border-ink bg-ground px-1 font-mono text-[10px] font-bold uppercase">
              {task.id}
            </code>
            {isActive(task.status) && (
              <span className="inline-block size-2 shrink-0 border border-ink bg-signal animate-pulse motion-reduce:animate-none" />
            )}
          </div>
          <p className="text-xs font-bold leading-snug mt-1 line-clamp-2 break-words">
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
        style: { stroke: "#000000", strokeWidth: 3 },
        markerEnd: {
          type: MarkerType.ArrowClosed,
          color: "#000000",
          width: 18,
          height: 18,
        },
      })),
    );
  }, [graph, setNodes, setEdges]);

  if (tasks.length === 0) {
    return (
      <div className="flex-1 min-h-0 flex items-center justify-center border-4 border-ink bg-contrast shadow-brutal-sm">
        <p className="px-6 text-center text-sm font-bold text-ink/60">
          no tasks yet — plan a feature to draw its graph
        </p>
      </div>
    );
  }

  return (
    <div className="flex-1 min-h-0 border-4 border-ink bg-ground shadow-brutal-sm overflow-hidden">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onNodeClick={(_, node) => select(node.id)}
        fitView
        minZoom={0.2}
        colorMode="light"
        // The DAG is the engine's, not the user's to rewire: pan, zoom, and
        // drag are fine, but nothing here can create or delete edges.
        nodesConnectable={false}
        deleteKeyCode={null}
      >
        <Background gap={20} color="#00000022" />
        <Controls showInteractive={false} />
      </ReactFlow>
    </div>
  );
}
