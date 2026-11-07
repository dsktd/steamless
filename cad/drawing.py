"""Draw the controller from Valve's Steam Controller shell STL.

    python drawing.py sc_solid_stl_20260429.stl out/

Writes controller.svg (the live-input drawing), icon.svg and icon-symbolic.svg (the app
icon) into out/. The STL is sampled into a depth map of the face (0.2 mm grid, grips down);
the shell outline and the steep walls around each control are traced into polygons and
simplified. Bumpers, triggers, grip sensors and back buttons have no outline from the top
and are placed by hand.
"""

import sys
from pathlib import Path

import numpy as np
from scipy import ndimage
from skimage import measure

# Grid step in mm, and drawing units per mm: the shell is 1000 units wide
STEP = 0.2
SCALE = 1000 / 158.755
# Room above the shell for the triggers and bumpers, in mm
TOP = 12.0
WIDTH, HEIGHT = 1000, 745
# A wall is steeper than this, in mm of depth per mm across the face
STEEP = 0.8

# Control centres in CAD mm, (X right, Z toward the grips), from Valve's reference drawing
CONTROLS = {
    "dpad": (-49.4, 19.7),
    "lstick": (-23.1, 31.0),
    "rstick": (22.6, 31.0),
    "btn-steam": (0.1, 19.2),
    "btn-view": (-28.4, 8.2),
    "btn-menu": (28.1, 8.2),
    "btn-y": (48.9, 10.2),
    "btn-x": (39.9, 19.5),
    "btn-b": (58.1, 19.5),
    "btn-a": (48.9, 28.7),
    "lpad": (-29.9, 61.2),
    "rpad": (29.9, 61.2),
    "btn-qam": (0.1, 61.7),
}

# The icon cuts these out of the shell
ICON_HOLES = ["lstick", "rstick", "btn-a", "btn-b", "btn-x", "btn-y", "lpad", "rpad"]

PLACED = """<rect id="lt" class="placed" x="120" y="10" width="150" height="34" rx="14"/>
<rect id="rt" class="placed" x="730" y="10" width="150" height="34" rx="14"/>
<rect id="lb" class="placed" x="95" y="42" width="200" height="30" rx="15"/>
<rect id="rb" class="placed" x="705" y="42" width="200" height="30" rx="15"/>
<clipPath id="clip-shell"><use href="#shell"/></clipPath>
<polygon id="grip-l-touch" class="placed" clip-path="url(#clip-shell)" points="0,400 180,400 180,560 245,600 245,790 0,790"/>
<polygon id="grip-r-touch" class="placed" clip-path="url(#clip-shell)" points="1000,400 820,400 820,560 755,600 755,790 1000,790"/>
<rect id="grip-l1" class="placed back" x="80" y="595" width="90" height="36" rx="12"/>
<rect id="grip-l2" class="placed back" x="80" y="645" width="90" height="36" rx="12"/>
<rect id="grip-r1" class="placed back" x="840" y="595" width="90" height="36" rx="12"/>
<rect id="grip-r2" class="placed back" x="840" y="645" width="90" height="36" rx="12"/>
"""

NOTICE = "Derived from Valve's Steam Controller shell CAD, CC BY-NC-SA 4.0; see LICENSE"


def triangles(path: Path) -> np.ndarray:
    """The binary STL's triangles, (n, 3, 3) float32."""
    data = np.fromfile(path, dtype=np.uint8)
    count = int(np.frombuffer(data[80:84], dtype=np.uint32)[0])
    record = np.dtype([("normal", "<f4", 3), ("v", "<f4", (3, 3)), ("attr", "<u2")])
    return np.frombuffer(data[84 : 84 + count * 50], dtype=record)["v"]


class Face:
    """The depth map of the face seen from the front: the highest Y per grid cell."""

    def __init__(self, tris: np.ndarray):
        flat = tris.reshape(-1, 3)
        self.x0 = float(flat[:, 0].min())
        self.z0 = float(flat[:, 2].min())
        # Vertices, edge midpoints and centroids, so every 0.2 mm cell gets a sample
        samples = np.concatenate(
            [
                flat,
                (tris[:, 0] + tris[:, 1]) / 2,
                (tris[:, 1] + tris[:, 2]) / 2,
                (tris[:, 2] + tris[:, 0]) / 2,
                tris.mean(axis=1),
            ]
        )
        col = ((samples[:, 0] - self.x0) / STEP).round().astype(int)
        row = ((samples[:, 2] - self.z0) / STEP).round().astype(int)
        depth = np.full((row.max() + 1, col.max() + 1), -np.inf)
        np.maximum.at(depth, (row, col), samples[:, 1])
        covered = ndimage.binary_fill_holes(
            ndimage.binary_closing(np.isfinite(depth), iterations=2)
        )
        # Fill the cells no sample hit with their neighbours' height
        filled = np.where(np.isfinite(depth), depth, -1e3)
        filled = np.where(
            np.isfinite(depth), depth, ndimage.grey_dilation(filled, size=3)
        )
        self.depth = np.where(covered, filled, np.nan)
        self.covered = covered

    def cell(self, x: float, z: float) -> tuple[int, int]:
        return round((z - self.z0) / STEP), round((x - self.x0) / STEP)

    def walls(self) -> np.ndarray:
        """Cells on a steep wall, or off the shell."""
        depth = np.nan_to_num(self.depth, nan=-1e3)
        dz, dx = np.gradient(depth, STEP)
        return (np.hypot(dx, dz) > STEEP) | ~self.covered

    def region(
        self,
        walls: np.ndarray,
        x: float,
        z: float,
    ) -> np.ndarray:
        """The flat cells around (x, z) inside the steep walls, holes filled."""
        labels, _ = ndimage.label(~walls)
        label = labels[self.cell(x, z)]
        if label == 0:
            raise ValueError(f"No flat surface at {x}, {z}")
        region = ndimage.binary_fill_holes(labels == label)
        # Take in the wall itself, so the outline sits on its top edge
        return ndimage.binary_dilation(region, iterations=2)

    def raised(
        self,
        x: float,
        z: float,
    ) -> np.ndarray:
        """The cells around (x, z) less than 4 mm below it: a stick's dished cap whole."""
        top = self.depth[self.cell(x, z)]
        labels, _ = ndimage.label(np.nan_to_num(self.depth, nan=-1e3) > top - 4)
        return ndimage.binary_fill_holes(labels == labels[self.cell(x, z)])

    def outline(self, mask: np.ndarray) -> np.ndarray:
        """The longest contour of a mask, in drawing units."""
        padded = np.pad(mask.astype(float), 1)
        contour = max(measure.find_contours(padded, 0.5), key=len) - 1
        rows, cols = contour[:, 0], contour[:, 1]
        x = (cols * STEP) * SCALE
        y = (rows * STEP + self.z0 + TOP) * SCALE
        return np.column_stack([x, y])


def simplify(points: np.ndarray, tolerance: float) -> np.ndarray:
    return measure.approximate_polygon(points, tolerance)


def path(points: np.ndarray) -> str:
    head, *rest = (f"{x:.1f},{y:.1f}" for x, y in points[:-1])
    return f"M{head} " + " ".join(f"L{p}" for p in rest) + " Z"


def dpad(region: np.ndarray, centre: tuple[int, int]) -> dict[str, np.ndarray]:
    """The cross split into its four arms along the diagonals."""
    rows, cols = np.indices(region.shape)
    dy, dx = rows - centre[0], cols - centre[1]
    vertical = abs(dy) >= abs(dx)
    return {
        "dpad-up": region & vertical & (dy < 0),
        "dpad-down": region & vertical & (dy >= 0),
        "dpad-left": region & ~vertical & (dx < 0),
        "dpad-right": region & ~vertical & (dx >= 0),
    }


def controls(face: Face) -> dict[str, np.ndarray]:
    walls = face.walls()
    shapes = {}
    for name, (x, z) in CONTROLS.items():
        if name.endswith("stick"):
            shapes[name] = simplify(face.outline(face.raised(x, z)), 0.6)
            continue
        region = face.region(walls, x, z)
        if name == "dpad":
            for arm, mask in dpad(region, face.cell(x, z)).items():
                shapes[arm] = simplify(face.outline(mask), 0.6)
            continue
        shapes[name] = simplify(face.outline(region), 0.6)
    return shapes


def drawing(
    shell: np.ndarray,
    shapes: dict[str, np.ndarray],
) -> str:
    lines = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {HEIGHT}" width="{WIDTH}" height="{HEIGHT}">',
        f"<!-- {NOTICE} -->",
        f'<path id="shell" class="shell" fill="none" d="{path(shell)}"/>',
    ]
    for name, points in shapes.items():
        lines.append(f'<path id="{name}" class="control" d="{path(points)}"/>')
    return "\n".join(lines) + "\n" + PLACED + "</svg>\n"


def icon(
    shell: np.ndarray,
    shapes: dict[str, np.ndarray],
) -> str:
    holes = "".join(f'<path d="{path(shapes[name])}"/>' for name in ICON_HOLES)
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">'
        f"<!-- {NOTICE} -->"
        '<rect width="512" height="512" rx="112" fill="#1e2230"/>'
        '<g transform="translate(26,108) scale(0.46)">'
        f'<path d="{path(shell)}" fill="#8fd3ff"/>\n'
        f'<g fill="#1e2230" stroke="#1e2230" stroke-width="10">{holes}</g>\n'
        '<g fill="#1e2230"><rect x="170" y="128" width="42" height="144" rx="10"/>'
        '<rect x="118" y="178" width="144" height="42" rx="10"/></g></g></svg>\n'
    )


def symbolic(
    shell: np.ndarray,
    shapes: dict[str, np.ndarray],
) -> str:
    """The 16 px outline with the trackpads cut out."""
    scale = 15 / WIDTH

    def small(points: np.ndarray) -> str:
        points = simplify(points, 6) * scale + [0.5, 8 - HEIGHT * scale / 2]
        return "M" + " ".join(f"{x:.2f},{y:.2f}" for x, y in points[:-1]) + "Z"

    d = " ".join(small(p) for p in (shell, shapes["lpad"], shapes["rpad"]))
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16">\n'
        f"<!-- {NOTICE} -->\n"
        f'<path fill="currentColor" fill-rule="evenodd" d="{d}"/>\n'
        "</svg>\n"
    )


def main():
    stl, out = Path(sys.argv[1]), Path(sys.argv[2])
    face = Face(triangles(stl))
    shell = simplify(face.outline(face.covered), 0.6)
    shapes = controls(face)
    out.mkdir(parents=True, exist_ok=True)
    (out / "controller.svg").write_text(drawing(shell, shapes))
    (out / "icon.svg").write_text(icon(shell, shapes))
    (out / "icon-symbolic.svg").write_text(symbolic(shell, shapes))


if __name__ == "__main__":
    main()
