// Client store: WebSocket lifecycle + REST actions over the pure reducer.
//
// The reducer (./reducer) owns state transitions; this file owns I/O and
// React binding. Keeping them separate means the merge/idempotency logic is
// testable without a DOM or a running daemon.

import { create } from "zustand";

import { api, connectEvents } from "./api";
import type { AttachmentPayload } from "./attachments";
import { applyEvent, countsFor, EMPTY_STATE, type BoardState } from "./reducer";
import type { ApiEvent, Task } from "./types";

/** Reconnect delay schedule (ms) after a WebSocket drop. */
const RECONNECT_STEPS = [500, 1_000, 2_000, 5_000];

/**
 * Render a caught value for the error banner.
 *
 * `ApiError` carries the daemon's message (e.g. a merge conflict's
 * "…approving t1: the approved diff does not apply…"), which is far more
 * useful than `String(err)`'s "ApiError: …" boilerplate.
 */
function errText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export interface ZouState extends BoardState {
  /** WebSocket status, drives the header's connection dot. */
  connection: "connecting" | "open" | "closed";
  /** Task currently open in the review/detail modal, if any. */
  selectedId: string | null;
  /** Non-fatal error banner text (API failures, WS decode errors). */
  error: string | null;
  /** True while a REST mutation is in flight (disables buttons). */
  busy: boolean;
  /** Kanban board or interactive DAG graph (Phase 11). */
  view: "board" | "dag";

  // --- lifecycle ---
  /** Connect to the daemon. Idempotent; safe to call from React effects. */
  connect: () => void;
  disconnect: () => void;
  /** Re-fetch tasks over REST (also used after mutations). */
  refresh: () => Promise<void>;

  // --- actions ---
  plan: (prompt: string, attachments?: AttachmentPayload[]) => Promise<void>;
  runAll: () => Promise<void>;
  runOne: (id: string) => Promise<void>;
  cancel: (id: string) => Promise<void>;
  deleteTask: (id: string) => Promise<void>;
  approve: (id: string) => Promise<void>;
  reject: (
    id: string,
    feedback: string,
    attachments?: AttachmentPayload[],
  ) => Promise<void>;
  /** Release a stuck IN_PROGRESS task back to TODO (crash recovery). */
  recover: (id: string) => Promise<void>;

  // --- view ---
  select: (id: string | null) => void;
  clearError: () => void;
  /** Switch between the kanban board and the DAG graph. */
  setView: (view: "board" | "dag") => void;
  /** Apply an event (exposed for tests; the store uses it internally). */
  dispatch: (ev: ApiEvent) => void;
}

// Module-scoped so `connect()` stays idempotent across StrictMode double-mounts
// and across multiple store consumers.
let socket: WebSocket | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let attempt = 0;
let manuallyClosed = false;

export const useZou = create<ZouState>((set, get) => {
  const dispatch = (ev: ApiEvent) => {
    set((state) => applyEvent(state, ev));
  };

  const openSocket = () => {
    if (typeof window === "undefined") return;
    set({ connection: "connecting" });

    const ws = connectEvents(
      (ev) => {
        // Any event means the link is alive.
        if (get().connection !== "open") set({ connection: "open" });
        attempt = 0;
        dispatch(ev);
      },
      (err) => set({ error: `bad event from daemon: ${errText(err)}` }),
    );
    socket = ws;

    ws.addEventListener("open", () => {
      set({ connection: "open" });
      attempt = 0;
    });

    ws.addEventListener("close", () => {
      socket = null;
      set({ connection: "closed" });
      if (manuallyClosed) return;
      // Back off, then reconnect. The daemon sends a fresh snapshot on
      // reconnect, so we never render a stale board.
      const delay =
        RECONNECT_STEPS[Math.min(attempt, RECONNECT_STEPS.length - 1)];
      attempt += 1;
      reconnectTimer = setTimeout(openSocket, delay);
    });

    ws.addEventListener("error", () => {
      // `close` always follows `error`; the reconnect logic lives there.
      set({ connection: "closed" });
    });
  };

  return {
    ...EMPTY_STATE,
    connection: "connecting",
    selectedId: null,
    error: null,
    busy: false,
    view: "board",

    connect: () => {
      if (socket || reconnectTimer) return;
      manuallyClosed = false;
      attempt = 0;
      openSocket();
      // Seed over REST too, so the board populates even before the socket's
      // first snapshot (or if the socket can't connect at all).
      void get().refresh();
    },

    disconnect: () => {
      manuallyClosed = true;
      if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      socket?.close();
      socket = null;
      set({ connection: "closed" });
    },

    refresh: async () => {
      try {
        const { project, tasks, dependencies } = await api.tasks();
        dispatch({
          type: "snapshot",
          project,
          tasks,
          dependencies,
          counts: countsFor(tasks),
        });
        set({ error: null });
      } catch (err) {
        set({ error: `failed to load tasks: ${errText(err)}` });
      }
    },

    plan: async (prompt, attachments = []) => {
      set({ busy: true });
      try {
        await api.plan(prompt, attachments);
        await get().refresh();
        set({ error: null });
      } catch (err) {
        set({ error: errText(err) });
      } finally {
        set({ busy: false });
      }
    },

    runAll: async () => {
      set({ busy: true });
      try {
        await api.runAll();
        await get().refresh();
        set({ error: null });
      } catch (err) {
        set({ error: errText(err) });
      } finally {
        set({ busy: false });
      }
    },

    runOne: async (id) => {
      set({ busy: true });
      try {
        await api.runOne(id);
        await get().refresh();
        set({ error: null });
      } catch (err) {
        set({ error: errText(err) });
      } finally {
        set({ busy: false });
      }
    },

    cancel: async (id) => {
      set({ busy: true });
      try {
        await api.cancel(id);
        await get().refresh();
        set({ error: null });
      } catch (err) {
        set({ error: errText(err) });
      } finally {
        set({ busy: false });
      }
    },

    deleteTask: async (id) => {
      set({ busy: true });
      try {
        await api.deleteTask(id);
        await get().refresh();
        set({
          error: null,
          selectedId: get().selectedId === id ? null : get().selectedId,
        });
      } catch (err) {
        set({ error: errText(err) });
      } finally {
        set({ busy: false });
      }
    },

    approve: async (id) => {
      set({ busy: true });
      try {
        const res = await api.approve(id);
        await get().refresh();
        // Phase 9: a conflict keeps the dialog open on the task, so the
        // reviewer watches the auto-resolution agent live instead of the
        // modal vanishing with the board showing an unexpected status.
        const queued = res.integration.status === "conflict_resolution_queued";
        set({ error: null, selectedId: queued ? id : null });
      } catch (err) {
        set({ error: errText(err) });
      } finally {
        set({ busy: false });
      }
    },

    reject: async (id, feedback, attachments = []) => {
      set({ busy: true });
      try {
        await api.reject(id, feedback, attachments);
        await get().refresh();
        set({ error: null, selectedId: null });
      } catch (err) {
        set({ error: errText(err) });
      } finally {
        set({ busy: false });
      }
    },

    recover: async (id) => {
      set({ busy: true });
      try {
        await api.recover(id);
        await get().refresh();
        // Keep the modal open: the task is still selected, just runnable now.
        set({ error: null });
      } catch (err) {
        set({ error: errText(err) });
      } finally {
        set({ busy: false });
      }
    },

    select: (id) => set({ selectedId: id }),
    clearError: () => set({ error: null }),
    setView: (view) => set({ view }),
    dispatch,
  };
});

/** Selectors / derived helpers used by components. */

export const selectTask = (id: string | null): Task | null => {
  if (!id) return null;
  return useZou.getState().tasks.find((t) => t.id === id) ?? null;
};
