import type { QrMatrix } from '@lan-meeting/contracts';

/**
 * A QR code, drawn from the module grid the backend produced.
 *
 * Rectangles rather than an injected SVG string or a `data:` image. The matrix
 * arrives as booleans and is rendered through React's own element creation, so
 * there is no `dangerouslySetInnerHTML` anywhere in the path and no new content
 * security policy allowance is needed (ADR-0017).
 *
 * Dark modules are drawn as filled squares on a light background. The light
 * background is explicit rather than inherited: a QR code on a dark surface does
 * not scan, so this must not follow the app's colour scheme.
 */
export function QrCode({
  matrix,
  size = 224,
  label,
}: {
  readonly matrix: QrMatrix;
  /** Rendered edge length in pixels. */
  readonly size?: number;
  readonly label: string;
}) {
  // One module of quiet zone on each side is the minimum a scanner needs to
  // find the code against its surroundings.
  const quiet = 1;
  const extent = matrix.size + quiet * 2;

  const squares: React.ReactElement[] = [];
  for (let row = 0; row < matrix.size; row += 1) {
    for (let column = 0; column < matrix.size; column += 1) {
      if (matrix.modules[row * matrix.size + column] === true) {
        squares.push(
          <rect
            key={`${row}-${column}`}
            x={column + quiet}
            y={row + quiet}
            width={1}
            height={1}
          />,
        );
      }
    }
  }

  return (
    <svg
      className="qr"
      width={size}
      height={size}
      viewBox={`0 0 ${extent} ${extent}`}
      role="img"
      aria-label={label}
      // `crispEdges` keeps module boundaries sharp when the code is scaled;
      // anti-aliased edges are what makes a small QR fail to scan.
      shapeRendering="crispEdges"
    >
      <rect width={extent} height={extent} fill="#ffffff" />
      <g fill="#000000">{squares}</g>
    </svg>
  );
}
