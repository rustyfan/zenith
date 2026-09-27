import argparse
import array
import json
import pathlib
import struct
import sys

import bpy
from mathutils import Vector


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    args = parser.parse_args(sys.argv[sys.argv.index("--") + 1:])
    output = pathlib.Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    center = Vector((0.0, -0.04, 1.64))
    scale = 10.0
    graph = bpy.context.evaluated_depsgraph_get()
    records = []
    summary = []
    for name in ["GEO-einar_head", "GEO-eye.L", "GEO-eye.R"]:
        source = bpy.data.objects[name].evaluated_get(graph)
        mesh = bpy.data.meshes.new_from_object(source)
        obj = bpy.data.objects.new("Zenith export", mesh)
        bpy.context.collection.objects.link(obj)
        modifier = obj.modifiers.new("Export subdivision", "SUBSURF")
        modifier.levels = 2
        bpy.context.view_layer.update()
        evaluated = obj.evaluated_get(bpy.context.evaluated_depsgraph_get())
        mesh = evaluated.to_mesh()
        mesh.calc_loop_triangles()
        vertices = array.array("f")
        indices = array.array("I")
        normal_matrix = source.matrix_world.to_3x3().inverted().transposed()
        for vertex in mesh.vertices:
            position = (source.matrix_world @ vertex.co - center) * scale
            normal = (normal_matrix @ vertex.normal).normalized()
            vertices.extend((*position, *normal, 0, 0, 0, 0, 0, 0))
        for triangle in mesh.loop_triangles:
            indices.extend(triangle.vertices)
        records.append((0 if name.endswith("head") else 1, vertices, indices))
        summary.append({"name": name, "vertices": len(vertices) // 12, "triangles": len(indices) // 3})
        evaluated.to_mesh_clear()
        bpy.data.objects.remove(obj, do_unlink=True)
    graph = bpy.context.evaluated_depsgraph_get()
    for obj in sorted(bpy.data.objects, key=lambda obj: obj.name):
        if obj.type != "CURVES" or not obj.name.startswith("GEO-einar_"):
            continue
        evaluated = obj.evaluated_get(graph)
        vertices = array.array("f")
        indices = array.array("I")
        strand_count = 0
        radius_scale = max(evaluated.matrix_world.to_scale()) * scale
        for curve in evaluated.data.curves:
            points = [(evaluated.matrix_world @ p.position - center) * scale for p in curve.points]
            if len(points) < 2:
                continue
            base = len(vertices) // 12
            lengths = [0.0]
            for a, b in zip(points, points[1:]):
                lengths.append(lengths[-1] + (b - a).length)
            if lengths[-1] < 1e-7:
                continue
            for i, point in enumerate(points):
                tangent = points[min(i + 1, len(points) - 1)] - points[max(i - 1, 0)]
                if tangent.length_squared < 1e-14:
                    tangent = Vector((0, 0, -1))
                tangent.normalize()
                side = tangent.cross(Vector((0, -1, 0)))
                if side.length_squared < 1e-8:
                    side = tangent.cross(Vector((1, 0, 0)))
                side.normalize()
                normal = side.cross(tangent).normalized()
                radius = max(curve.points[i].radius * radius_scale, 1e-6)
                for edge in range(2):
                    position = point + side * (2 * edge - 1) * radius
                    vertices.extend((*position, *normal, edge, lengths[i] / lengths[-1], *tangent, 1))
                if i:
                    a = base + (i - 1) * 2
                    indices.extend((a, a + 1, a + 2, a + 1, a + 3, a + 2))
            strand_count += 1
        if strand_count:
            records.append((2, vertices, indices))
            summary.append({"name": obj.name, "strands": strand_count, "vertices": len(vertices) // 12, "triangles": len(indices) // 3})
            print(summary[-1], flush=True)
    assert any(kind == 2 for kind, _, _ in records), "No strand geometry found"
    with output.open("wb") as stream:
        stream.write(b"ZEINAR01" + struct.pack("<I", len(records)))
        for kind, vertices, indices in records:
            stream.write(struct.pack("<III", kind, len(vertices) // 12, len(indices)))
            if sys.byteorder != "little":
                vertices.byteswap()
                indices.byteswap()
            vertices.tofile(stream)
            indices.tofile(stream)
    output.with_suffix(".json").write_text(json.dumps({
        "credit": "Einar Rig (CC-BY) Blender Foundation | studio.blender.org",
        "source": "https://studio.blender.org/characters/einar/v1/",
        "changes": "Static subdivided head and eyes; evaluated curves converted to ribbons; neutral materials; centered and scaled 10x.",
        "meshes": summary,
    }, indent=2), encoding="utf-8")
    print("Exported", output, output.stat().st_size, "bytes", flush=True)


main()
