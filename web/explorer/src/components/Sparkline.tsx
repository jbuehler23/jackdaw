// Sparkline.tsx: canvas history chart, ported from the PoC's drawSpark.
// Area fill under the line, three faint gridlines, a highlighted endpoint dot.
import { useEffect, useRef } from 'preact/hooks';

export function Sparkline({ data, color }: { data: number[]; color: string }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    const w = canvas.width;
    const h = canvas.height;
    ctx.clearRect(0, 0, w, h);
    if (data.length < 2) return;

    const max = Math.max(...data) * 1.15 || 1;
    const min = Math.min(0, ...data);

    ctx.strokeStyle = 'rgba(65,65,66,0.5)';
    ctx.lineWidth = 1;
    for (const frac of [0.25, 0.5, 0.75]) {
      ctx.beginPath();
      ctx.moveTo(0, h * frac);
      ctx.lineTo(w, h * frac);
      ctx.stroke();
    }

    const points = data.map((v, i): [number, number] => [
      (i / (data.length - 1)) * w,
      h - ((v - min) / (max - min)) * (h - 8) - 4,
    ]);

    ctx.beginPath();
    ctx.moveTo(points[0][0], h);
    for (const [x, y] of points) ctx.lineTo(x, y);
    ctx.lineTo(points[points.length - 1][0], h);
    ctx.closePath();
    ctx.fillStyle = `${color}2E`;
    ctx.fill();

    ctx.beginPath();
    points.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
    ctx.strokeStyle = color;
    ctx.lineWidth = 2;
    ctx.lineJoin = 'round';
    ctx.stroke();

    const [ex, ey] = points[points.length - 1];
    ctx.beginPath();
    ctx.arc(ex - 1, ey, 3, 0, Math.PI * 2);
    ctx.fillStyle = color;
    ctx.fill();
  }, [data, color]);

  return <canvas ref={canvasRef} width={400} height={92} />;
}
