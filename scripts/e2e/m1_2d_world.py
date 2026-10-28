#!/usr/bin/env python3
"""M1 end-to-end: build and Play a 2D world scene over the editor's BRP surface.

Each step maps to an MCP tool (call_operator, screenshot, wait, save_scene,
scene_tree, get_entity), which forward to the same BRP methods used here.

Run:
  1. Launch an editor on a project whose assets/tiny_farm/tilemap_packed.png
     exists and whose toolchain can build the game (needed for Play):
       JACKDAW_OPEN_PROJECT=<project> JACKDAW_REMOTE_PORT=<port> jackdaw
  2. python3 scripts/e2e/m1_2d_world.py --project <project> --port <port> \
         --out <output dir>

Play shows its first frame slowly on a software renderer; raise --play-settle
if m1-play.png still says "Waiting for the game's first frame".

Writes <out>/m1-editor.png and <out>/m1-play.png, and a scene file
<project>/assets/<--scene> that it deletes unless --keep-scene is given.
Exits non-zero with a message on the first failed step.
"""

import argparse
import json
import os
import shutil
import struct
import sys
import time
import urllib.request

SHEET = "tiny_farm/tilemap_packed.png"
TILE = 16
PPU = 16
PROJECTION = "bevy_camera::projection::Projection"
SPRITE = "bevy_sprite::sprite::Sprite"
PLAY_TIMEOUT = 900
GAME_SETTLE = 90


class Failure(Exception):
    pass


class Editor:
    def __init__(self, project, port):
        self.project = project
        self.url = f"http://127.0.0.1:{port}"
        self.counter = 0
        self.scratch = []

    def brp(self, method, params=None, timeout=120):
        body = {"jsonrpc": "2.0", "id": 1, "method": method}
        if params is not None:
            body["params"] = params
        req = urllib.request.Request(
            self.url,
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            reply = json.loads(resp.read())
        if "error" in reply:
            raise Failure(f"{method}: {reply['error']}")
        return reply["result"]

    def operator(self, op_id, **params):
        result = self.brp("jackdaw/call_operator", {"id": op_id, "params": params})
        if result.get("result") != "finished":
            raise Failure(f"{op_id} {params}: {result}")
        return result

    def wait(self, until=None, frames=None, timeout=120):
        if until:
            return self.brp("jackdaw/wait", {"until": until}, timeout=timeout)
        self.counter += 1
        return self.brp(
            "jackdaw/wait", {"frames": frames or 1, "request": f"m1-{self.counter}"}
        )

    def screenshot(self, kind, path, region=None):
        self.counter += 1
        params = {"kind": kind, "path": path, "request": f"m1-shot-{self.counter}"}
        if region:
            params["region"] = region
        self.brp("jackdaw/screenshot", params)
        full = os.path.join(self.project, path)
        self.scratch.append(full)
        deadline = time.time() + 30
        while not os.path.exists(full) and time.time() < deadline:
            time.sleep(0.2)
        time.sleep(1.5)
        return full

    def status(self):
        return self.brp("jackdaw/status")

    def bsn(self):
        return self.brp("jackdaw/scene_bsn")["bsn"]


def check(condition, message):
    if not condition:
        raise Failure(message)


def png_size(path):
    with open(path, "rb") as file:
        head = file.read(24)
    check(head[:8] == b"\x89PNG\r\n\x1a\n", f"{path} is not a PNG")
    return struct.unpack(">II", head[16:24])


def entity_id(result, what):
    entities = result.get("entities") or []
    check(entities, f"{what}: operator returned no entity id")
    return entities[0]


def tile_rect(col, row):
    return json.dumps(
        {
            "min": [col * TILE, row * TILE],
            "max": [(col + 1) * TILE, (row + 1) * TILE],
        }
    )


def create_scene(ed, scene_path):
    ed.operator("scene.new", kind="2d", path=scene_path)
    ed.wait(frames=3)
    status = ed.status()
    check(status["scene"].endswith(os.path.basename(scene_path)), f"scene: {status}")


def set_project_units(ed):
    ed.operator("project.set_pixels_per_unit", value=PPU)
    ed.operator("project.set_2d_sampling", mode="nearest")


def add_tile_sprite(ed, name, col, row, x, y):
    result = ed.operator("sprite.add", path=SHEET, name=name, x=x, y=y)
    entity = entity_id(result, name)
    ed.operator(
        "field.set", entity=entity, type_path=SPRITE, field="rect",
        value=tile_rect(col, row),
    )
    ed.operator(
        "field.set", entity=entity, type_path=SPRITE, field="custom_size",
        value=json.dumps([1, 1]),
    )
    return entity


def add_sprites(ed):
    return {
        "Farmer": add_tile_sprite(ed, "Farmer", 1, 9, 0, 0),
        "Tree A": add_tile_sprite(ed, "Tree A", 3, 1, -3, 1),
        "Tree B": add_tile_sprite(ed, "Tree B", 3, 2, 3, 1),
        "Rocks": add_tile_sprite(ed, "Rocks", 5, 6, -2, -2),
        "Sunflower": add_tile_sprite(ed, "Sunflower", 11, 6, 2, -2),
        "Axe": add_tile_sprite(ed, "Axe", 3, 7, 0, -3),
    }


def nudge_farmer(ed, farmer):
    ed.operator("selection.select", name="Farmer")
    ed.operator("entity.set_transform", entity=farmer, x=0.5, y=-0.5)
    tree = ed.brp("jackdaw/entity", {"entity": farmer})["bsn"]
    check("x: 0.5" in tree, f"farmer transform not nudged: {tree[:400]}")


def sort_sprites(ed, sprites):
    for name in ("Farmer", "Tree A", "Tree B"):
        ed.operator("sprite.set_anchor", entity=sprites[name], preset="bottom_center")
        ed.operator("sprite.y_sort", entity=sprites[name], on=True)
    ed.operator("selection.select", name="Axe")
    ed.operator("sprite.z_up")


def add_pixel_camera(ed):
    result = ed.operator("entity.add.pixel_camera")
    camera = entity_id(result, "pixel camera")
    ed.operator("camera.pixel_canvas", entity=camera, width=320, height=180)
    # The game does not know the editor's pixels per unit, so the camera
    # zooms in by it to draw one world unit as 16 canvas pixels.
    ed.operator(
        "field.set", entity=camera, type_path=PROJECTION, field="0.scale",
        value=str(1 / PPU),
    )


def frame_view(ed):
    ed.operator("viewport2d.frame")
    ed.wait(frames=5)
    ed.operator("viewport2d.pixel_perfect")
    ed.wait(frames=5)


def editor_shot(ed, out):
    region = {"x": -3.6, "y": -3.7, "w": 7.2, "h": 5.9, "space": "world"}
    rel = ".jackdaw/m1-editor.png"
    path = ed.screenshot("viewport2d", rel, region)
    check(os.path.exists(path), f"screenshot missing: {path}")
    width, height = png_size(path)
    check(width > 0 and height > 0, "empty region screenshot")
    check(
        abs(width / height - 7.2 / 5.9) < 0.05,
        f"region aspect {width}x{height} does not match the 7.2x5.9 world region",
    )
    dest = os.path.join(out, "m1-editor.png")
    shutil.copyfile(path, dest)
    return dest, (width, height)


def save_scene(ed, scene_path):
    if os.path.exists(scene_path):
        os.remove(scene_path)
    ed.operator("scene.save")
    deadline = time.time() + 30
    while not os.path.exists(scene_path) and time.time() < deadline:
        time.sleep(0.3)
    check(os.path.exists(scene_path), f"scene not saved to {scene_path}")
    time.sleep(1)
    with open(scene_path) as file:
        text = file.read()
    check(text.count("YSort") >= 3, "saved scene lacks three YSort components")
    check("PixelCanvas" in text, "saved scene lacks PixelCanvas")
    check(text.count("rect:") >= 6, "saved scene lacks the sprite rects")
    check("Scene2dRoot" in text, "saved scene lacks Scene2dRoot")
    check("z: 1.0" in text or "z: 2.0" in text, "saved scene shows no stepped z")
    return text


def wait_for_play(ed):
    deadline = time.time() + PLAY_TIMEOUT
    idle_since = None
    while time.time() < deadline:
        status = ed.status()
        if status["pie"] == "running":
            return
        busy = status["progress"] is not None or status["pie"] != "stopped"
        idle_since = None if busy else (idle_since or time.time())
        if idle_since and time.time() - idle_since > 20:
            raise Failure(f"pie.play did not start or build: {status}")
        time.sleep(1)
    raise Failure(f"Play not running after {PLAY_TIMEOUT}s: {ed.status()}")


def play_and_shoot(ed, out, settle):
    ed.operator("pie.play")
    wait_for_play(ed)
    time.sleep(settle)
    rel = ".jackdaw/m1-play.png"
    path = ed.screenshot("window", rel)
    check(os.path.exists(path), f"play screenshot missing: {path}")
    width, height = png_size(path)
    check(width >= 640 and height >= 360, f"play screenshot too small {width}x{height}")
    dest = os.path.join(out, "m1-play.png")
    shutil.copyfile(path, dest)
    return dest, (width, height)


def stop_play(ed):
    ed.operator("pie.stop")
    deadline = time.time() + 60
    while ed.status()["pie"] != "stopped" and time.time() < deadline:
        time.sleep(0.5)
    check(ed.status()["pie"] == "stopped", f"Play did not stop: {ed.status()}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--project", required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--scene", default="m1_2d_world.bsn")
    parser.add_argument(
        "--play-settle", type=float, default=GAME_SETTLE,
        help="seconds to let the game stream its first frame before the capture",
    )
    parser.add_argument("--keep-scene", action="store_true")
    args = parser.parse_args()

    project = os.path.abspath(args.project)
    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)
    scene_path = os.path.join(project, "assets", args.scene)
    ed = Editor(project, args.port)
    sprites = {}

    def step(name, fn, *fn_args):
        print(f"[ .. ] {name}", flush=True)
        result = fn(*fn_args)
        print(f"[ ok ] {name}", flush=True)
        return result

    try:
        status = ed.status()
        check(status["pie"] == "stopped", f"editor is not idle: {status}")
        step("new 2d scene", create_scene, ed, scene_path)
        step("ppu 16 and nearest sampling", set_project_units, ed)
        sprites = step("add tile sprites", add_sprites, ed)
        step("nudge farmer", nudge_farmer, ed, sprites["Farmer"])
        step("anchor, y-sort, z step", sort_sprites, ed, sprites)
        step("pixel camera 320x180", add_pixel_camera, ed)
        step("frame and pixel perfect", frame_view, ed)
        shot, size = step("editor screenshot", editor_shot, ed, out)
        print(f"       {shot} {size[0]}x{size[1]}")
        step("save scene", save_scene, ed, scene_path)
        shot, size = step("play and screenshot", play_and_shoot, ed, out, args.play_settle)
        print(f"       {shot} {size[0]}x{size[1]}")
        step("stop play", stop_play, ed)
    except Failure as err:
        print(f"[FAIL] {err}", file=sys.stderr)
        return 1
    finally:
        try:
            if ed.status()["pie"] != "stopped":
                stop_play(ed)
        except Exception:
            pass
        time.sleep(1.5)
        for leftover in ed.scratch:
            if os.path.exists(leftover):
                os.remove(leftover)
        if not args.keep_scene and os.path.exists(scene_path):
            os.remove(scene_path)
    print("M1 end-to-end passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
