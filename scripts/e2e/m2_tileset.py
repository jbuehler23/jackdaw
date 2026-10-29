#!/usr/bin/env python3
"""M2 end-to-end: import, review and hand-edit a tileset over the editor's BRP surface.

Each step maps to an MCP tool (call_operator, screenshot), which forward to the
same BRP methods used here.

Run:
  1. Launch an editor on a project whose assets/tiny_farm/tilemap_packed.png
     exists:
       JACKDAW_OPEN_PROJECT=<project> JACKDAW_REMOTE_PORT=<port> jackdaw
  2. python3 scripts/e2e/m2_tileset.py --project <project> --port <port> \
         --out <output dir>

Writes <out>/m2-tileset.png. The tileset, its packed copy and meta go to
<project>/assets/<--folder>, which it deletes unless --keep is given.
Exits non-zero with a message on the first failed step.
"""

import argparse
import json
import os
import re
import shutil
import struct
import sys
import time
import urllib.request

SHEET = "tiny_farm/tilemap_packed.png"
GRASS, SAND, DIRT = 1, 2, 3


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
        params = {k: str(v) for k, v in params.items()}
        result = self.brp("jackdaw/call_operator", {"id": op_id, "params": params})
        if result.get("result") != "finished":
            raise Failure(f"{op_id} {params}: {result}")
        if result.get("warnings"):
            raise Failure(f"{op_id} {params} warned: {result['warnings']}")
        return result

    def wait(self, frames=1):
        self.counter += 1
        return self.brp(
            "jackdaw/wait", {"frames": frames, "request": f"m2-{self.counter}"}
        )

    def screenshot(self, kind, path):
        self.counter += 1
        params = {"kind": kind, "path": path, "request": f"m2-shot-{self.counter}"}
        self.brp("jackdaw/screenshot", params)
        full = os.path.join(self.project, path)
        self.scratch.append(full)
        wait_stable(full)
        return full


def wait_stable(path, timeout=30, polls=5):
    """Wait until `path` exists with a size unchanged for `polls` polls."""
    deadline = time.time() + timeout
    last, same = None, 0
    while time.time() < deadline:
        size = os.path.getsize(path) if os.path.exists(path) else None
        same = same + 1 if size and size == last else 0
        if same >= polls:
            return
        last = size
        time.sleep(0.2)
    raise Failure(f"{path} did not settle within {timeout}s")


def check(condition, message):
    if not condition:
        raise Failure(message)


def png_size(path):
    with open(path, "rb") as file:
        head = file.read(24)
    check(head[:8] == b"\x89PNG\r\n\x1a\n", f"{path} is not a PNG")
    return struct.unpack(">II", head[16:24])


class Tileset:
    """The tileset under test: its asset path and file on disk."""

    def __init__(self, ed, folder):
        self.ed = ed
        self.path = f"{folder}/farm.tileset.bsn"
        self.file = os.path.join(ed.project, "assets", self.path)

    def text(self):
        with open(self.file) as file:
            return file.read()

    def edit(self, op_id, **params):
        return self.ed.operator(op_id, path=self.path, **params)

    def wait_for_change(self, before, what):
        deadline = time.time() + 20
        while time.time() < deadline:
            if os.path.exists(self.file) and self.text() != before:
                return self.text()
            self.ed.wait(frames=2)
        raise Failure(f"{what}: {self.path} did not change")

    def describe(self):
        reports = self.edit("tileset.describe").get("reports") or []
        check(reports, "tileset.describe returned no report")
        return "\n".join(reports)

    def tile_of(self, terrain):
        pattern = r"sheet: 0,\s*tile: (\d+),\s*peers: \[([^\]]*)\]"
        for tile, peers in re.findall(pattern, self.text()):
            ids = [int(p) for p in re.findall(r"\d+", peers)]
            if terrain in ids:
                return int(tile)
        raise Failure(f"no tile of terrain {terrain} in {self.path}")


def import_auto(ed, ts):
    ed.operator("tileset.import", path=SHEET, out=ts.path, setup="auto")
    deadline = time.time() + 30
    while not os.path.exists(ts.file) and time.time() < deadline:
        ed.wait(frames=2)
    check(os.path.exists(ts.file), f"import wrote no {ts.file}")
    text = ts.describe()
    for line in (
        "terrains: 3",
        '  terrain 1 "grass": swatch #84c669, background',
        '  terrain 2 "sand path": swatch #e7a16a',
        '  terrain 3 "dirt path": swatch #cb7c51',
        "terrain sets: 2",
        "  set 2 over 1: ",
        "  set 3 over 1: ",
    ):
        check(line in text, f"import report lacks {line!r}:\n{text}")


def rename_and_undo(ed, ts):
    imported = ts.text()
    ts.edit("tileset.terrain.add", name="water", swatch="#3366ff")
    before = ts.wait_for_change(imported, "terrain.add")
    ts.edit("tileset.terrain.rename", terrain="4", name="pond")
    renamed = ts.wait_for_change(before, "terrain.rename")
    check("pond" in renamed, "rename did not write the new name")
    ed.operator("history.undo")
    restored = ts.wait_for_change(renamed, "undo")
    check(restored == before, "undo did not restore the tileset file")


def terrain_edits(ed, ts):
    sand = ts.tile_of(SAND)
    ts.edit("tileset.terrain.split", terrain=str(SAND), tiles=f"0:{sand}", name="wet sand")
    ts.edit("tileset.terrain.merge", **{"from": "wet sand", "into": str(SAND)})
    ts.edit("tileset.terrain.set_collision", terrain=str(DIRT), mode="solid")
    ts.edit("tileset.terrain_set.add", inner="4", outer=str(GRASS), mode="sides")
    ts.edit("tileset.terrain_set.set", inner="4", outer=str(GRASS), mode="corners")
    ts.edit(
        "tileset.apply_template", inner=str(DIRT), outer=str(SAND),
        template="corner2x2", origin="10,0",
    )


def tile_edits(ed, ts):
    tile = ts.tile_of(SAND)
    ts.edit("tileset.tile.set_bits", tile=f"0:{tile}", peer="centre", terrain=str(DIRT))
    ts.edit("tileset.tile.reset_to_auto", tiles=f"0:{tile}")
    ts.edit("tileset.paint_property", tiles="3", key="water", value="deep")
    ts.edit("tileset.object.add", origin="0,10", size="1,1", pivot="0.5,0")
    ts.edit("tileset.object.remove", origin="0,10")
    ts.edit("tileset.terrain.remove", terrain="water")


def check_final(ed, ts):
    text = ts.describe()
    for line in (
        "terrains: 3",
        '  terrain 3 "dirt path": swatch #cb7c51, collision solid',
        "properties: 1 layers",
    ):
        check(line in text, f"final report lacks {line!r}:\n{text}")
    check("water" not in text.split("properties")[0], f"water terrain kept:\n{text}")


def dock_shot(ed, out):
    ed.wait(frames=20)
    path = ed.screenshot("window", ".jackdaw/m2-tileset.png")
    check(os.path.exists(path), f"screenshot missing: {path}")
    width, height = png_size(path)
    check(width >= 640 and height >= 360, f"screenshot too small {width}x{height}")
    dest = os.path.join(out, "m2-tileset.png")
    shutil.copyfile(path, dest)
    return dest, (width, height)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--project", required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--folder", default="m2_tileset")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()

    project = os.path.abspath(args.project)
    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)
    folder = os.path.join(project, "assets", args.folder)
    check_fresh = not os.path.exists(folder)
    ed = Editor(project, args.port)
    ts = Tileset(ed, args.folder)

    def step(name, fn, *fn_args, **fn_kwargs):
        print(f"[ .. ] {name}", flush=True)
        result = fn(*fn_args, **fn_kwargs)
        print(f"[ ok ] {name}", flush=True)
        return result

    try:
        check(check_fresh, f"{folder} already exists")
        step("import with auto setup", import_auto, ed, ts)
        step("open the tileset window", ed.operator, "tileset.edit", path=ts.path)
        step("rename, then undo restores the file", rename_and_undo, ed, ts)
        step("terrain and set operators", terrain_edits, ed, ts)
        step("tile, property and object operators", tile_edits, ed, ts)
        step("describe the result", check_final, ed, ts)
        shot, size = step("dock screenshot", dock_shot, ed, out)
        print(f"       {shot} {size[0]}x{size[1]}")
    except Failure as err:
        print(f"[FAIL] {err}", file=sys.stderr)
        return 1
    finally:
        for leftover in ed.scratch:
            if os.path.exists(leftover):
                try:
                    wait_stable(leftover)
                except Failure:
                    pass
                os.remove(leftover)
        if check_fresh and not args.keep and os.path.exists(folder):
            shutil.rmtree(folder)
    print("M2 end-to-end passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
