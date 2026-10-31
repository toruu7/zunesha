// Prompt attachments for the playground.
//
// Mirrors `web/src/lib/attachments.ts`; the two frontends build separately, so
// this small module is duplicated rather than introducing a shared package for
// one utility. Limits match `zunesha-core::attachments`.

export const MAX_ATTACHMENTS = 10;
export const MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024;
export const MAX_TOTAL_ATTACHMENT_BYTES = 24 * 1024 * 1024;

export interface AttachmentDraft {
  name: string;
  media_type: string;
  data: string;
  size: number;
  preview?: string;
}

export interface AttachmentPayload {
  name: string;
  media_type: string;
  data: string;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function readFile(file: File): Promise<AttachmentDraft> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.addEventListener('error', () => reject(reader.error ?? new Error('read failed')));
    reader.addEventListener('load', () => {
      const result = String(reader.result ?? '');
      const comma = result.indexOf(',');
      if (comma < 0) {
        reject(new Error(`could not read ${file.name}`));
        return;
      }
      const mediaType = file.type || 'application/octet-stream';
      resolve({
        name: file.name || 'attachment',
        media_type: mediaType,
        data: result.slice(comma + 1),
        size: file.size,
        preview: mediaType.startsWith('image/') ? result : undefined,
      });
    });
    reader.readAsDataURL(file);
  });
}

export async function addAttachmentFiles(
  existing: AttachmentDraft[],
  files: Iterable<File>,
): Promise<{ attachments: AttachmentDraft[]; errors: string[] }> {
  const next = [...existing];
  const errors: string[] = [];
  let total = next.reduce((sum, attachment) => sum + attachment.size, 0);

  for (const file of files) {
    if (next.length >= MAX_ATTACHMENTS) {
      errors.push(`Only ${MAX_ATTACHMENTS} attachments are allowed.`);
      break;
    }
    if (file.size === 0) {
      errors.push(`${file.name || 'attachment'} is empty.`);
      continue;
    }
    if (file.size > MAX_ATTACHMENT_BYTES) {
      errors.push(`${file.name} is larger than ${formatBytes(MAX_ATTACHMENT_BYTES)}.`);
      continue;
    }
    if (total + file.size > MAX_TOTAL_ATTACHMENT_BYTES) {
      errors.push(`Attachments would exceed ${formatBytes(MAX_TOTAL_ATTACHMENT_BYTES)} in total.`);
      break;
    }
    try {
      const attachment = await readFile(file);
      next.push(attachment);
      total += file.size;
    } catch (error) {
      errors.push(error instanceof Error ? error.message : String(error));
    }
  }
  return { attachments: next, errors };
}

export function toPayload(attachments: AttachmentDraft[]): AttachmentPayload[] {
  return attachments.map(({ name, media_type, data }) => ({ name, media_type, data }));
}
