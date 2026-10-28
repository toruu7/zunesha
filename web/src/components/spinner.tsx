/**
 * A small square-ish spinner for live worker states.
 *
 * The ring inherits `currentColor`, so the same component works on the light
 * cards and the ink column headers. Reduced-motion users still get the ring
 * (the top gap is the signal) without the animation.
 */
export default function Spinner({
  className = "size-3",
  label = "running",
}: {
  className?: string;
  label?: string;
}) {
  return (
    <span
      role="status"
      aria-label={label}
      className={`inline-block shrink-0 animate-spin rounded-full border-2 border-current border-t-transparent motion-reduce:animate-none ${className}`}
    />
  );
}
