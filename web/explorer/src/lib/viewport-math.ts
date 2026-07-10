// viewport-math.ts: pure vector/camera/projection math for the canvas viewport.
// No canvas or DOM access here; components own drawing, this module owns the numbers.
// Ported from the PoC's orbit camera (.scratch/web-explorer/jackdaw-explorer-poc.html).

export interface Vec3 {
  x: number;
  y: number;
  z: number;
}

export interface CameraState {
  target: Vec3;
  yaw: number;
  pitch: number;
  dist: number;
}

export interface CamBasis {
  eye: Vec3;
  fwd: Vec3;
  right: Vec3;
  up: Vec3;
}

export interface Face {
  depth: number;
  pts: { x: number; y: number }[];
  shade: number;
}

export interface GizmoAxis {
  axis: 'x' | 'y' | 'z';
  dir: Vec3;
  o: { x: number; y: number };
  tip: { x: number; y: number };
  len: number;
}

const VFOV = 0.45; // radians, ~52 degree vertical field of view
const NEAR = 0.15;
const PITCH_MIN = 0.05;
const PITCH_MAX = 1.45;
const DIST_MIN = 4;
const DIST_MAX = 90;
const ORBIT_SENSITIVITY = 0.008;
const PAN_SENSITIVITY = 0.0016;
const ZOOM_BASE = 1.0012;

const vsub = (a: Vec3, b: Vec3): Vec3 => ({ x: a.x - b.x, y: a.y - b.y, z: a.z - b.z });
const vadd = (a: Vec3, b: Vec3): Vec3 => ({ x: a.x + b.x, y: a.y + b.y, z: a.z + b.z });
const vscale = (a: Vec3, s: number): Vec3 => ({ x: a.x * s, y: a.y * s, z: a.z * s });
const vdot = (a: Vec3, b: Vec3): number => a.x * b.x + a.y * b.y + a.z * b.z;
const vcross = (a: Vec3, b: Vec3): Vec3 => ({
  x: a.y * b.z - a.z * b.y,
  y: a.z * b.x - a.x * b.z,
  z: a.x * b.y - a.y * b.x,
});
const vnorm = (a: Vec3): Vec3 => {
  const l = Math.hypot(a.x, a.y, a.z) || 1;
  return vscale(a, 1 / l);
};

export function camBasis(cam: CameraState): CamBasis {
  const dir: Vec3 = {
    x: Math.cos(cam.pitch) * Math.sin(cam.yaw),
    y: Math.sin(cam.pitch),
    z: Math.cos(cam.pitch) * Math.cos(cam.yaw),
  };
  const eye = vadd(cam.target, vscale(dir, cam.dist));
  const fwd = vnorm(vsub(cam.target, eye));
  const right = vnorm(vcross(fwd, { x: 0, y: 1, z: 0 }));
  const up = vcross(right, fwd);
  return { eye, fwd, right, up };
}

export function project(
  p: Vec3,
  basis: CamBasis,
  width: number,
  height: number,
): { x: number; y: number; z: number } | null {
  const v = vsub(p, basis.eye);
  const z = vdot(v, basis.fwd);
  if (z < NEAR) return null;
  const f = height / 2 / Math.tan(VFOV);
  return {
    x: width / 2 + (vdot(v, basis.right) * f) / z,
    y: height / 2 - (vdot(v, basis.up) * f) / z,
    z,
  };
}

export function orbit(cam: CameraState, dx: number, dy: number): CameraState {
  return {
    ...cam,
    yaw: cam.yaw - dx * ORBIT_SENSITIVITY,
    pitch: Math.max(PITCH_MIN, Math.min(PITCH_MAX, cam.pitch + dy * ORBIT_SENSITIVITY)),
  };
}

export function pan(cam: CameraState, basis: CamBasis, dx: number, dy: number): CameraState {
  const s = cam.dist * PAN_SENSITIVITY;
  const target = vadd(cam.target, vadd(vscale(basis.right, -dx * s), vscale(basis.up, dy * s)));
  return { ...cam, target };
}

export function zoom(cam: CameraState, deltaY: number): CameraState {
  const dist = Math.max(DIST_MIN, Math.min(DIST_MAX, cam.dist * Math.pow(ZOOM_BASE, deltaY)));
  return { ...cam, dist };
}

const LIGHT_DIR = vnorm({ x: 0.5, y: 0.85, z: 0.25 });

const BOX_FACES: { n: Vec3; idx: [number, number, number, number] }[] = [
  { n: { x: 1, y: 0, z: 0 }, idx: [1, 2, 6, 5] },
  { n: { x: -1, y: 0, z: 0 }, idx: [0, 4, 7, 3] },
  { n: { x: 0, y: 1, z: 0 }, idx: [4, 5, 6, 7] },
  { n: { x: 0, y: -1, z: 0 }, idx: [0, 3, 2, 1] },
  { n: { x: 0, y: 0, z: 1 }, idx: [3, 7, 6, 2] },
  { n: { x: 0, y: 0, z: -1 }, idx: [0, 1, 5, 4] },
];

function boxCorners(c: Vec3, h: Vec3): Vec3[] {
  const out: Vec3[] = [];
  for (const dy of [-1, 1]) {
    for (const dz of [-1, 1]) {
      for (const dx of dz === -1 ? [-1, 1] : [1, -1]) {
        out.push({ x: c.x + dx * h.x, y: c.y + dy * h.y, z: c.z + dz * h.z });
      }
    }
  }
  return out;
}

export function boxFaces(center: Vec3, half: Vec3, basis: CamBasis, w: number, h: number): Face[] {
  const corners = boxCorners(center, half).map((p) => project(p, basis, w, h));
  if (corners.some((c) => !c)) return [];
  const projected = corners as { x: number; y: number; z: number }[];

  const faces: Face[] = [];
  for (const face of BOX_FACES) {
    const faceCenter = vadd(center, { x: face.n.x * half.x, y: face.n.y * half.y, z: face.n.z * half.z });
    if (vdot(face.n, vsub(faceCenter, basis.eye)) >= 0) continue; // backface
    const pts = face.idx.map((i) => ({ x: projected[i].x, y: projected[i].y }));
    const depth = face.idx.reduce((sum, i) => sum + projected[i].z, 0) / 4;
    const shade = 0.48 + 0.52 * Math.max(0, vdot(face.n, LIGHT_DIR));
    faces.push({ depth, pts, shade });
  }
  faces.sort((a, b) => b.depth - a.depth);
  return faces;
}

const GIZMO_AXES: { axis: 'x' | 'y' | 'z'; dir: Vec3 }[] = [
  { axis: 'x', dir: { x: 1, y: 0, z: 0 } },
  { axis: 'y', dir: { x: 0, y: 1, z: 0 } },
  { axis: 'z', dir: { x: 0, y: 0, z: 1 } },
];

export function axisGizmo(pos: Vec3, len: number, basis: CamBasis, w: number, h: number): GizmoAxis[] {
  const origin = project(pos, basis, w, h);
  if (!origin) return [];
  const axes: GizmoAxis[] = [];
  for (const a of GIZMO_AXES) {
    const tip = project(vadd(pos, vscale(a.dir, len)), basis, w, h);
    if (!tip) continue;
    axes.push({ axis: a.axis, dir: a.dir, o: { x: origin.x, y: origin.y }, tip: { x: tip.x, y: tip.y }, len });
  }
  return axes;
}

export function axisDragDelta(axis: GizmoAxis, dxPixels: number, dyPixels: number): number {
  const screenDir = { x: axis.tip.x - axis.o.x, y: axis.tip.y - axis.o.y };
  const lenSq = screenDir.x * screenDir.x + screenDir.y * screenDir.y || 1;
  const along = (dxPixels * screenDir.x + dyPixels * screenDir.y) / Math.sqrt(lenSq);
  const worldPerPixel = axis.len / Math.sqrt(lenSq);
  return along * worldPerPixel;
}
