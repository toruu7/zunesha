// Speech bubble renderer

const SCALE = 3;
const MAX_CHARS = 40;
const PADDING = 4;

export function drawBubble(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  text: string,
  type: 'info' | 'error' | 'success',
): void {
  const s = SCALE;
  const bx = x * s;
  const by = y * s - 10 * s;

  // Word wrap
  const lines = wrapText(text, MAX_CHARS);
  const lineCount = Math.min(lines.length, 3);

  // Measure max width
  ctx.font = `${3 * s}px monospace`;
  let maxWidth = 0;
  for (const line of lines.slice(0, lineCount)) {
    const w = ctx.measureText(line).width;
    if (w > maxWidth) maxWidth = w;
  }

  const bubbleW = maxWidth + PADDING * 2 * s;
  const totalH = lineCount * 4 * s + PADDING * 2 * s;

  // Position bubble centered above character
  const bLeft = bx - bubbleW / 2 + 8 * s;
  const bTop = by - totalH;

  // Background
  ctx.fillStyle = type === 'error' ? '#FFE0E0' : type === 'success' ? '#E0FFE0' : '#FFFFF0';
  ctx.strokeStyle = type === 'error' ? '#CC3333' : type === 'success' ? '#33AA33' : '#888';
  ctx.lineWidth = 1;

  // Rounded rect
  roundRect(ctx, bLeft, bTop, bubbleW, totalH, 3 * s);

  // Pointer triangle
  ctx.fillStyle = type === 'error' ? '#FFE0E0' : type === 'success' ? '#E0FFE0' : '#FFFFF0';
  ctx.beginPath();
  ctx.moveTo(bx + 4 * s, bTop + totalH);
  ctx.lineTo(bx + 8 * s, bTop + totalH + 3 * s);
  ctx.lineTo(bx + 12 * s, bTop + totalH);
  ctx.fill();

  // Border for pointer
  ctx.strokeStyle = type === 'error' ? '#CC3333' : type === 'success' ? '#33AA33' : '#888';
  ctx.beginPath();
  ctx.moveTo(bx + 4 * s, bTop + totalH);
  ctx.lineTo(bx + 8 * s, bTop + totalH + 3 * s);
  ctx.lineTo(bx + 12 * s, bTop + totalH);
  ctx.stroke();

  // Text
  ctx.fillStyle = type === 'error' ? '#991111' : '#333';
  ctx.font = `${3 * s}px monospace`;
  ctx.textAlign = 'left';
  for (let i = 0; i < lineCount; i++) {
    ctx.fillText(lines[i], bLeft + PADDING * s, bTop + PADDING * s + (i + 1) * 4 * s);
  }
  if (lines.length > lineCount) {
    ctx.fillText('...', bLeft + PADDING * s, bTop + PADDING * s + (lineCount + 1) * 4 * s - 2 * s);
  }
}

function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
): void {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.lineTo(x + w - r, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + r);
  ctx.lineTo(x + w, y + h - r);
  ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
  ctx.lineTo(x + r, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - r);
  ctx.lineTo(x, y + r);
  ctx.quadraticCurveTo(x, y, x + r, y);
  ctx.closePath();
  ctx.fill();
  ctx.stroke();
}

function wrapText(text: string, maxChars: number): string[] {
  const words = text.split(' ');
  const lines: string[] = [];
  let current = '';

  for (const word of words) {
    if (current.length + word.length + 1 > maxChars) {
      if (current) lines.push(current);
      current = word;
    } else {
      current = current ? `${current} ${word}` : word;
    }
  }
  if (current) lines.push(current);
  return lines;
}
