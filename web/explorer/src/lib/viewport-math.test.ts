import { describe, expect, it } from 'vitest';
import { axisDragDelta, axisGizmo, boxFaces, camBasis, orbit, pan, project, zoom, type CameraState } from './viewport-math';

const baseCam: CameraState = { target: { x: 0, y: 0, z: 0 }, yaw: 0, pitch: 0.62, dist: 10 };

describe('camBasis + project', () => {
  it('projects a point directly in front of the camera to canvas center', () => {
    const cam: CameraState = { target: { x: 0, y: 0, z: 0 }, yaw: 0, pitch: 0, dist: 10 };
    const basis = camBasis(cam);
    // eye sits at (0, 0, dist) looking toward target when yaw=pitch=0, so the
    // target itself lies directly ahead of the camera.
    const p = project(cam.target, basis, 800, 600);
    expect(p).not.toBeNull();
    expect(p!.x).toBeCloseTo(400, 5);
    expect(p!.y).toBeCloseTo(300, 5);
  });

  it('returns null for a point behind the camera', () => {
    const cam: CameraState = { target: { x: 0, y: 0, z: 0 }, yaw: 0, pitch: 0, dist: 10 };
    const basis = camBasis(cam);
    const behind = { x: basis.eye.x - basis.fwd.x, y: basis.eye.y - basis.fwd.y, z: basis.eye.z - basis.fwd.z };
    expect(project(behind, basis, 800, 600)).toBeNull();
  });
});

describe('orbit', () => {
  it('changes yaw and pitch and clamps pitch within [0.05, 1.45]', () => {
    const rotated = orbit(baseCam, 100, 50);
    expect(rotated.yaw).toBeCloseTo(baseCam.yaw - 100 * 0.008, 10);
    expect(rotated.pitch).toBeCloseTo(baseCam.pitch + 50 * 0.008, 10);

    const clampedHigh = orbit(baseCam, 0, 100000);
    expect(clampedHigh.pitch).toBeCloseTo(1.45, 10);
    const clampedLow = orbit(baseCam, 0, -100000);
    expect(clampedLow.pitch).toBeCloseTo(0.05, 10);
  });

  it('leaves the other camera fields untouched', () => {
    const rotated = orbit(baseCam, 10, 10);
    expect(rotated.target).toEqual(baseCam.target);
    expect(rotated.dist).toBe(baseCam.dist);
  });
});

describe('pan', () => {
  it('moves the target along the camera right/up plane', () => {
    const basis = camBasis(baseCam);
    const panned = pan(baseCam, basis, 10, 0);
    expect(panned.target).not.toEqual(baseCam.target);
    expect(panned.dist).toBe(baseCam.dist);
  });
});

describe('zoom', () => {
  it('clamps dist within [4, 90]', () => {
    const zoomedOut = zoom({ ...baseCam, dist: 89 }, 100000);
    expect(zoomedOut.dist).toBeCloseTo(90, 10);
    const zoomedIn = zoom({ ...baseCam, dist: 5 }, -100000);
    expect(zoomedIn.dist).toBeCloseTo(4, 10);
  });

  it('scales dist by 1.0012^deltaY', () => {
    const cam = { ...baseCam, dist: 10 };
    const zoomed = zoom(cam, 100);
    expect(zoomed.dist).toBeCloseTo(10 * Math.pow(1.0012, 100), 10);
  });
});

describe('boxFaces', () => {
  it('returns 3 visible faces for a unit cube in front of the camera, sorted farthest first', () => {
    const cam: CameraState = { target: { x: 0, y: 0, z: 0 }, yaw: 0.6, pitch: 0.3, dist: 10 };
    const basis = camBasis(cam);
    const faces = boxFaces({ x: 0, y: 0, z: 0 }, { x: 0.5, y: 0.5, z: 0.5 }, basis, 800, 600);
    expect(faces.length).toBe(3);
    for (let i = 1; i < faces.length; i++) {
      expect(faces[i - 1].depth).toBeGreaterThanOrEqual(faces[i].depth);
    }
    for (const face of faces) {
      expect(face.pts.length).toBe(4);
      expect(face.shade).toBeGreaterThanOrEqual(0.48);
      expect(face.shade).toBeLessThanOrEqual(1);
    }
  });
});

describe('axisGizmo + axisDragDelta', () => {
  it('produces a positive delta when dragging along the axis screen direction', () => {
    const cam: CameraState = { target: { x: 0, y: 0, z: -5 }, yaw: 0, pitch: 0.3, dist: 10 };
    const basis = camBasis(cam);
    const axes = axisGizmo({ x: 0, y: 0, z: -5 }, 1, basis, 800, 600);
    expect(axes.length).toBeGreaterThan(0);
    const xAxis = axes.find((a) => a.axis === 'x')!;
    expect(xAxis).toBeDefined();
    const screenDir = { x: xAxis.tip.x - xAxis.o.x, y: xAxis.tip.y - xAxis.o.y };
    // Dragging exactly along the axis' own screen direction should read as a
    // positive delta (moving the object further along its positive axis).
    const delta = axisDragDelta(xAxis, screenDir.x, screenDir.y);
    expect(delta).toBeGreaterThan(0);
    const reverseDelta = axisDragDelta(xAxis, -screenDir.x, -screenDir.y);
    expect(reverseDelta).toBeLessThan(0);
  });
});
