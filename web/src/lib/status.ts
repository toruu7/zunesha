// Per-status presentation shared by the kanban board and the DAG view.
//
// One source of truth for what each status looks like, so the two views can
// never drift apart. Fills come from the press-sheet palette only; the status
// label always carries the meaning, so colour is an accent rather than the
// sole signal (black and white are legible at any size, REVIEW and
// RESOLVING_CONFLICTS share red, and FAILED uses its deep variant).

import type { TaskStatus } from "./types";

/** Per-status fill (Tailwind class) for rails, dots, and swatches. */
export const ACCENT: Record<TaskStatus, string> = {
  BACKLOG: "bg-contrast",
  TODO: "bg-emphasis",
  IN_PROGRESS: "bg-field",
  EVALUATING: "bg-ground",
  REVIEW: "bg-signal",
  RESOLVING_CONFLICTS: "bg-signal",
  DONE: "bg-ink",
  FAILED_NEEDS_HUMAN: "bg-signal-ink",
};

/** Statuses a live worker currently owns (pulsing dot). */
export function isActive(status: TaskStatus): boolean {
  return (
    status === "IN_PROGRESS" ||
    status === "EVALUATING" ||
    status === "RESOLVING_CONFLICTS"
  );
}
