"use client";

import { useCallback, useRef, useState } from "react";

import {
  addAttachmentFiles,
  type AttachmentDraft,
  type AttachmentPayload,
  toPayload,
} from "./attachments";

export function useAttachments() {
  const [attachments, setAttachments] = useState<AttachmentDraft[]>([]);
  const [errors, setErrors] = useState<string[]>([]);
  // File reads are async; keep the latest list in a ref so two quick
  // drops/pastes cannot each start from a stale render snapshot.
  const current = useRef<AttachmentDraft[]>([]);
  // Serialize file reads: two quick drops must each see the previous result,
  // not both start from the same pre-drop snapshot.
  const queue = useRef<Promise<void>>(Promise.resolve());

  const commit = useCallback((next: AttachmentDraft[]) => {
    current.current = next;
    setAttachments(next);
  }, []);

  const add = useCallback(
    (files: Iterable<File>) => {
      const run = queue.current.then(async () => {
        const result = await addAttachmentFiles(current.current, files);
        commit(result.attachments);
        setErrors(result.errors);
        return result;
      });
      queue.current = run.then(
        () => undefined,
        () => undefined,
      );
      return run;
    },
    [commit],
  );

  const remove = useCallback(
    (index: number) => {
      commit(current.current.filter((_, i) => i !== index));
      setErrors([]);
    },
    [commit],
  );

  const clear = useCallback(() => {
    commit([]);
    setErrors([]);
  }, [commit]);

  const payload = useCallback((): AttachmentPayload[] => toPayload(current.current), []);

  return { attachments, errors, add, remove, clear, payload };
}
