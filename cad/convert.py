"""Convert Valve's Steam Controller shell STL to the 3D view's binary glTF.

    blender -b --python convert.py -- sc_solid_stl_20260429.stl controller.glb 0.04

Decimates to the given ratio (0.04: about 63,000 triangles), smooth shades, and moves CAD
millimetres (X right, Y out of the face, Z toward the grips) to Bevy (Y up, Z toward the
viewer), centred, in units of 10 cm: (X, -(Z - 50), Y + 24) / 100. yakin-view's cad() is
the same map.
"""

import sys

import bpy
from mathutils import Matrix

src, dst, ratio = sys.argv[sys.argv.index("--") + 1 :]
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.wm.stl_import(filepath=src)
ob = bpy.context.selected_objects[0]
bpy.context.view_layer.objects.active = ob

# Merge the STL's split vertices so the decimator sees one surface
bpy.ops.object.mode_set(mode="EDIT")
bpy.ops.mesh.select_all(action="SELECT")
bpy.ops.mesh.remove_doubles(threshold=0.001)
bpy.ops.object.mode_set(mode="OBJECT")

mod = ob.modifiers.new("dec", "DECIMATE")
mod.ratio = float(ratio)
bpy.ops.object.modifier_apply(modifier="dec")
bpy.ops.object.shade_auto_smooth(angle=0.6)

ob.data.transform(
    Matrix(((0.01, 0, 0, 0), (0, 0, -0.01, 0.5), (0, 0.01, 0, 0.24), (0, 0, 0, 1)))
)
print("TRIS", sum(len(p.vertices) - 2 for p in ob.data.polygons))
bpy.ops.export_scene.gltf(
    filepath=dst,
    export_format="GLB",
    export_yup=False,
    export_apply=True,
    export_normals=True,
    export_materials="NONE",
    export_draco_mesh_compression_enable=False,
)
