
pub const NODE_TRIANGLE: u32 = 0;
pub const NODE_BOX16: u32 = 4;
pub const NODE_BOX32: u32 = 5;
pub const NODE_INSTANCE: u32 = 6;
pub const NODE_AABB: u32 = 7;
pub const INVALID_NODE: u32 = 0xffff_ffff;

pub const HEADER_SIZE: usize = 128;
pub const BOX32_SIZE: usize = 128;
pub const TRI_NODE_SIZE: usize = 64;
pub const AABB_NODE_SIZE: usize = 64;
pub const INSTANCE_NODE_SIZE: usize = 128;

pub const AS_TYPE_TOP_LEVEL: u32 = 0;
pub const AS_TYPE_BOTTOM_LEVEL: u32 = 1;
pub const GEOM_TRIANGLES: u32 = 0;
pub const GEOM_AABBS: u32 = 1;
pub const GEOM_FLAG_OPAQUE: u32 = 1;

pub const DXGI_FORMAT_R32G32B32_FLOAT: u32 = 6;
pub const DXGI_FORMAT_R32G32_FLOAT: u32 = 16;
pub const DXGI_FORMAT_R16G16B16A16_FLOAT: u32 = 10;
pub const DXGI_FORMAT_R32_UINT: u32 = 42;
pub const DXGI_FORMAT_R16_UINT: u32 = 57;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct PrebuildInfo {
    pub result_data_max_size: u64,
    pub scratch_data_size: u64,
    pub update_scratch_data_size: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };

    pub fn from_triangle(v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> Self {
        let mut b = Self::EMPTY;
        b.grow_point(v0);
        b.grow_point(v1);
        b.grow_point(v2);
        b
    }

    pub fn grow_point(&mut self, p: [f32; 3]) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(p[i]);
            self.max[i] = self.max[i].max(p[i]);
        }
    }

    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1]), self.min[2].min(other.min[2])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1]), self.max[2].max(other.max[2])],
        }
    }

    pub fn centroid(&self) -> [f32; 3] {
        [
            0.5 * (self.min[0] + self.max[0]),
            0.5 * (self.min[1] + self.max[1]),
            0.5 * (self.min[2] + self.max[2]),
        ]
    }

    pub fn extent(&self) -> [f32; 3] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1], self.max[2] - self.min[2]]
    }

    pub fn transform(&self, m: &[f32; 12]) -> Self {
        if !self.min[0].is_finite() {
            return Self::EMPTY;
        }
        let mut out = Self::EMPTY;
        for mask in 0..8u32 {
            let p = [
                if mask & 1 == 0 { self.min[0] } else { self.max[0] },
                if mask & 2 == 0 { self.min[1] } else { self.max[1] },
                if mask & 4 == 0 { self.min[2] } else { self.max[2] },
            ];
            out.grow_point(transform_point(m, p));
        }
        out
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TriangleInput {
    pub v: [[f32; 3]; 3],
    pub geometry_index: u32,
    pub primitive_index: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct InstanceInput {
    pub transform: [f32; 12],
    pub instance_id: u32,
    pub mask: u32,
    pub sbt_offset: u32,
    pub flags: u32,
    pub blas_va: u64,
    pub blas_bounds: Aabb,
}

#[derive(Clone, Debug)]
pub struct BuiltBvh {
    pub bytes: Vec<u8>,
    pub bounds: Aabb,
    pub leaf_count: u32,
    pub internal_count: u32,
    pub root_node: u32,
}

pub fn align_up(v: u64, align: u64) -> u64 {
    (v + align - 1) & !(align - 1)
}

pub fn encode_node_id(byte_offset: usize, node_type: u32) -> u32 {
    debug_assert_eq!(byte_offset & 63, 0);
    (((byte_offset as u64) >> 3) as u32) | (node_type & 7)
}

pub fn transform_point(m: &[f32; 12], p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
        m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
        m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11],
    ]
}

pub fn invert_3x4(m: &[f32; 12]) -> [f32; 12] {
    let (a00, a01, a02, tx) = (m[0] as f64, m[1] as f64, m[2] as f64, m[3] as f64);
    let (a10, a11, a12, ty) = (m[4] as f64, m[5] as f64, m[6] as f64, m[7] as f64);
    let (a20, a21, a22, tz) = (m[8] as f64, m[9] as f64, m[10] as f64, m[11] as f64);

    let c00 = a11 * a22 - a12 * a21;
    let c01 = a12 * a20 - a10 * a22;
    let c02 = a10 * a21 - a11 * a20;
    let det = a00 * c00 + a01 * c01 + a02 * c02;
    if det.abs() < 1e-20 || !det.is_finite() {
        return [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    }
    let inv = 1.0 / det;
    let r00 = c00 * inv;
    let r01 = (a02 * a21 - a01 * a22) * inv;
    let r02 = (a01 * a12 - a02 * a11) * inv;
    let r10 = c01 * inv;
    let r11 = (a00 * a22 - a02 * a20) * inv;
    let r12 = (a02 * a10 - a00 * a12) * inv;
    let r20 = c02 * inv;
    let r21 = (a01 * a20 - a00 * a21) * inv;
    let r22 = (a00 * a11 - a01 * a10) * inv;

    let itx = -(r00 * tx + r01 * ty + r02 * tz);
    let ity = -(r10 * tx + r11 * ty + r12 * tz);
    let itz = -(r20 * tx + r21 * ty + r22 * tz);
    [
        r00 as f32, r01 as f32, r02 as f32, itx as f32,
        r10 as f32, r11 as f32, r12 as f32, ity as f32,
        r20 as f32, r21 as f32, r22 as f32, itz as f32,
    ]
}

pub fn prebuild_info(as_type: u32, leaf_count: u64) -> PrebuildInfo {
    let leaves = leaf_count.max(1);
    let internal_nodes = leaves.max(1);
    let leaf_stride = if as_type == AS_TYPE_TOP_LEVEL { INSTANCE_NODE_SIZE as u64 } else { TRI_NODE_SIZE as u64 };
    let result = align_up(
        HEADER_SIZE as u64 + internal_nodes * BOX32_SIZE as u64 + leaves * leaf_stride,
        256,
    );
    let scratch = align_up(result.max(4096), 256);
    let update = align_up((result / 2).max(4096), 256);
    PrebuildInfo {
        result_data_max_size: result,
        scratch_data_size: scratch,
        update_scratch_data_size: update,
    }
}

pub unsafe fn prebuild_info_from_inputs(inputs: *const u8) -> PrebuildInfo {
    if inputs.is_null() {
        return prebuild_info(AS_TYPE_BOTTOM_LEVEL, 0);
    }
    let rd32 = |o: usize| unsafe { core::ptr::read_unaligned(inputs.add(o) as *const u32) };
    let rd64 = |o: usize| unsafe { core::ptr::read_unaligned(inputs.add(o) as *const u64) };
    let as_type = rd32(0);
    let num_descs = rd32(8) as u64;
    let layout = rd32(12); // 0 = ARRAY, 1 = ARRAY_OF_POINTERS
    if as_type == AS_TYPE_TOP_LEVEL {
        return prebuild_info(AS_TYPE_TOP_LEVEL, num_descs);
    }
    let ptr = rd64(16);
    if ptr == 0 || num_descs == 0 {
        return prebuild_info(AS_TYPE_BOTTOM_LEVEL, 0);
    }
    let mut total_prims = 0u64;
    for i in 0..num_descs as usize {
        let gptr = if layout == 1 {
            unsafe { core::ptr::read_unaligned((ptr as *const u64).add(i)) as *const u8 }
        } else {
            unsafe { (ptr as *const u8).add(i * 56) }
        };
        if gptr.is_null() {
            continue;
        }
        let gtype = unsafe { core::ptr::read_unaligned(gptr as *const u32) };
        if gtype == GEOM_TRIANGLES {
            let idx_cnt = unsafe { core::ptr::read_unaligned(gptr.add(0x18) as *const u32) } as u64;
            let vtx_cnt = unsafe { core::ptr::read_unaligned(gptr.add(0x1c) as *const u32) } as u64;
            let prims = if idx_cnt != 0 { idx_cnt / 3 } else { vtx_cnt / 3 };
            total_prims = total_prims.saturating_add(prims);
        } else if gtype == GEOM_AABBS {
            let aabb_cnt = unsafe { core::ptr::read_unaligned(gptr.add(0x08) as *const u64) };
            total_prims = total_prims.saturating_add(aabb_cnt);
        }
    }
    prebuild_info(AS_TYPE_BOTTOM_LEVEL, total_prims)
}

fn wr32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn wr64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

fn wrf32(buf: &mut [u8], off: usize, v: f32) {
    wr32(buf, off, v.to_bits());
}

fn write_header(buf: &mut [u8], bounds: Aabb, leaf_count: u32, internal_count: u32, as_type: u32) {
    let bvh_off = if leaf_count == 0 { 0u32 } else { HEADER_SIZE as u32 };
    wr32(buf, 0x00, bvh_off);
    wr32(buf, 0x04, 0);
    for i in 0..3 {
        wrf32(buf, 0x08 + i * 4, bounds.min[i]);
        wrf32(buf, 0x14 + i * 4, bounds.max[i]);
    }
    let sz = buf.len() as u64;
    wr64(buf, 0x20, sz);
    wr64(buf, 0x28, sz);
    wr64(buf, 0x38, sz);
    wr32(buf, 0x40, internal_count);
    wr32(buf, 0x44, leaf_count);
    wr32(buf, 0x48, as_type);
}

struct LeafEntry {
    bounds: Aabb,
    centroid: [f32; 3],
    node_id: u32,
}

fn write_box32(buf: &mut [u8], off: usize, children: [u32; 4], bounds: [Aabb; 4]) {
    for i in 0..4 {
        wr32(buf, off + i * 4, children[i]);
        let bo = off + 16 + i * 24;
        if children[i] == INVALID_NODE {
            wrf32(buf, bo, f32::NAN);
            for k in 1..6 {
                wrf32(buf, bo + k * 4, 0.0);
            }
        } else {
            for k in 0..3 {
                wrf32(buf, bo + k * 4, bounds[i].min[k]);
                wrf32(buf, bo + 12 + k * 4, bounds[i].max[k]);
            }
        }
    }
}

fn partition4(items: &mut [LeafEntry]) -> Vec<core::ops::Range<usize>> {
    if items.len() <= 4 {
        return (0..items.len()).map(|i| i..i + 1).collect();
    }
    let split_once = |slice: &mut [LeafEntry]| -> usize {
        let mut cb = Aabb::EMPTY;
        for e in slice.iter() {
            cb.grow_point(e.centroid);
        }
        let ext = cb.extent();
        let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] {
            0
        } else if ext[1] >= ext[2] {
            1
        } else {
            2
        };
        let mid = slice.len() / 2;
        slice.select_nth_unstable_by(mid, |a, b| a.centroid[axis].total_cmp(&b.centroid[axis]));
        mid.max(1)
    };
    let m = split_once(items);
    let mut ranges = Vec::with_capacity(4);
    if m > 1 {
        let l = split_once(&mut items[..m]);
        ranges.push(0..l);
        ranges.push(l..m);
    } else {
        ranges.push(0..m);
    }
    let rlen = items.len() - m;
    if rlen > 1 {
        let r = split_once(&mut items[m..]);
        ranges.push(m..m + r);
        ranges.push(m + r..items.len());
    } else {
        ranges.push(m..items.len());
    }
    ranges
}

fn build_qbvh4_recursive(buf: &mut Vec<u8>, items: &mut [LeafEntry], internal_count: &mut u32) -> (u32, Aabb) {
    let box_off = buf.len();
    buf.resize(box_off + BOX32_SIZE, 0);
    *internal_count += 1;

    let mut children = [INVALID_NODE; 4];
    let mut cbounds = [Aabb::EMPTY; 4];
    let mut total = Aabb::EMPTY;

    let parts = partition4(items);
    for (slot, range) in parts.into_iter().enumerate() {
        let sub = &mut items[range];
        let (cid, cb) = if sub.len() == 1 {
            (sub[0].node_id, sub[0].bounds)
        } else {
            build_qbvh4_recursive(buf, sub, internal_count)
        };
        children[slot] = cid;
        cbounds[slot] = cb;
        total = total.union(&cb);
    }
    write_box32(buf, box_off, children, cbounds);
    (encode_node_id(box_off, NODE_BOX32), total)
}

pub fn build_blas(tris: &[TriangleInput]) -> BuiltBvh {
    if tris.is_empty() {
        let mut bytes = vec![0u8; align_up((HEADER_SIZE + BOX32_SIZE) as u64, 256) as usize];
        write_box32(&mut bytes, HEADER_SIZE, [INVALID_NODE; 4], [Aabb::EMPTY; 4]);
        write_header(&mut bytes, Aabb::EMPTY, 0, 1, AS_TYPE_BOTTOM_LEVEL);
        return BuiltBvh {
            bytes,
            bounds: Aabb::EMPTY,
            leaf_count: 0,
            internal_count: 1,
            root_node: encode_node_id(HEADER_SIZE, NODE_BOX32),
        };
    }

    let mut leaves: Vec<LeafEntry> = Vec::with_capacity(tris.len());
    let mut leaf_bytes = vec![0u8; tris.len() * TRI_NODE_SIZE];
    for (i, t) in tris.iter().enumerate() {
        let off = i * TRI_NODE_SIZE;
        for (vi, p) in t.v.iter().enumerate() {
            for k in 0..3 {
                wrf32(&mut leaf_bytes, off + vi * 12 + k * 4, p[k]);
            }
        }
        let opaque = if t.flags & GEOM_FLAG_OPAQUE != 0 { 1u32 << 31 } else { 0 };
        wr32(&mut leaf_bytes, off + 0x30, i as u32);
        wr32(&mut leaf_bytes, off + 0x34, (t.geometry_index & 0x0fff_ffff) | opaque);
        wr32(&mut leaf_bytes, off + 0x3c, t.primitive_index);

        let b = Aabb::from_triangle(t.v[0], t.v[1], t.v[2]);
        leaves.push(LeafEntry {
            bounds: b,
            centroid: b.centroid(),
            node_id: (i as u32) | 0x8000_0000,
        });
    }

    let mut bytes = vec![0u8; HEADER_SIZE];
    let mut internal_count = 0u32;
    let (root_node, total_bounds) = build_qbvh4_recursive(&mut bytes, &mut leaves, &mut internal_count);
    let leaf_base = bytes.len();
    debug_assert_eq!(leaf_base & 63, 0);
    bytes.extend_from_slice(&leaf_bytes);

    for n in 0..internal_count as usize {
        let box_off = HEADER_SIZE + n * BOX32_SIZE;
        for s in 0..4 {
            let co = box_off + s * 4;
            let cid = u32::from_le_bytes(bytes[co..co + 4].try_into().unwrap());
            if cid != INVALID_NODE && (cid & 0x8000_0000) != 0 {
                let leaf_idx = (cid & 0x7fff_ffff) as usize;
                wr32(&mut bytes, co, encode_node_id(leaf_base + leaf_idx * TRI_NODE_SIZE, NODE_TRIANGLE));
            }
        }
    }

    let aligned_len = align_up(bytes.len() as u64, 256) as usize;
    bytes.resize(aligned_len, 0);
    write_header(&mut bytes, total_bounds, tris.len() as u32, internal_count, AS_TYPE_BOTTOM_LEVEL);
    BuiltBvh {
        bytes,
        bounds: total_bounds,
        leaf_count: tris.len() as u32,
        internal_count,
        root_node,
    }
}

pub fn build_tlas(instances: &[InstanceInput]) -> BuiltBvh {
    if instances.is_empty() {
        let mut bytes = vec![0u8; align_up((HEADER_SIZE + BOX32_SIZE) as u64, 256) as usize];
        write_box32(&mut bytes, HEADER_SIZE, [INVALID_NODE; 4], [Aabb::EMPTY; 4]);
        write_header(&mut bytes, Aabb::EMPTY, 0, 1, AS_TYPE_TOP_LEVEL);
        return BuiltBvh {
            bytes,
            bounds: Aabb::EMPTY,
            leaf_count: 0,
            internal_count: 1,
            root_node: encode_node_id(HEADER_SIZE, NODE_BOX32),
        };
    }

    let mut leaves: Vec<LeafEntry> = Vec::with_capacity(instances.len());
    let mut leaf_bytes = vec![0u8; instances.len() * INSTANCE_NODE_SIZE];
    for (i, inst) in instances.iter().enumerate() {
        let off = i * INSTANCE_NODE_SIZE;
        let blas_ptr = (inst.blas_va >> 3) | ((inst.flags as u64 & 0xff) << 54);
        wr64(&mut leaf_bytes, off + 0x00, blas_ptr);
        wr32(&mut leaf_bytes, off + 0x08, (inst.instance_id & 0x00ff_ffff) | ((inst.mask & 0xff) << 24));
        wr32(&mut leaf_bytes, off + 0x0c, (inst.sbt_offset & 0x00ff_ffff) | ((inst.flags & 0xff) << 24));
        let wto = invert_3x4(&inst.transform);
        for k in 0..12 {
            wrf32(&mut leaf_bytes, off + 0x10 + k * 4, wto[k]);
        }
        wr32(&mut leaf_bytes, off + 0x40, i as u32);
        wr32(&mut leaf_bytes, off + 0x44, HEADER_SIZE as u32);
        for k in 0..12 {
            wrf32(&mut leaf_bytes, off + 0x50 + k * 4, inst.transform[k]);
        }

        let wb = inst.blas_bounds.transform(&inst.transform);
        leaves.push(LeafEntry {
            bounds: wb,
            centroid: wb.centroid(),
            node_id: (i as u32) | 0x8000_0000,
        });
    }

    let mut bytes = vec![0u8; HEADER_SIZE];
    let mut internal_count = 0u32;
    let (root_node, total_bounds) = build_qbvh4_recursive(&mut bytes, &mut leaves, &mut internal_count);
    let leaf_base = bytes.len();
    debug_assert_eq!(leaf_base & 63, 0);
    bytes.extend_from_slice(&leaf_bytes);

    for n in 0..internal_count as usize {
        let box_off = HEADER_SIZE + n * BOX32_SIZE;
        for s in 0..4 {
            let co = box_off + s * 4;
            let cid = u32::from_le_bytes(bytes[co..co + 4].try_into().unwrap());
            if cid != INVALID_NODE && (cid & 0x8000_0000) != 0 {
                let leaf_idx = (cid & 0x7fff_ffff) as usize;
                wr32(&mut bytes, co, encode_node_id(leaf_base + leaf_idx * INSTANCE_NODE_SIZE, NODE_INSTANCE));
            }
        }
    }

    let aligned_len = align_up(bytes.len() as u64, 256) as usize;
    bytes.resize(aligned_len, 0);
    write_header(&mut bytes, total_bounds, instances.len() as u32, internal_count, AS_TYPE_TOP_LEVEL);
    BuiltBvh {
        bytes,
        bounds: total_bounds,
        leaf_count: instances.len() as u32,
        internal_count,
        root_node,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rd32(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }

    fn rdf32(b: &[u8], o: usize) -> f32 {
        f32::from_bits(rd32(b, o))
    }

    fn ray_aabb(o: [f32; 3], inv: [f32; 3], tmax: f32, mn: [f32; 3], mx: [f32; 3]) -> Option<f32> {
        if mn[0].is_nan() {
            return None;
        }
        let mut lo = 0.0f32;
        let mut hi = tmax;
        for k in 0..3 {
            let t0 = (mn[k] - o[k]) * inv[k];
            let t1 = (mx[k] - o[k]) * inv[k];
            lo = lo.max(t0.min(t1));
            hi = hi.min(t0.max(t1));
        }
        (hi >= lo && lo < tmax).then_some(lo)
    }

    fn ray_tri(o: [f32; 3], d: [f32; 3], tmax: f32, v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> Option<f32> {
        let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let e1 = sub(v1, v0);
        let e2 = sub(v2, v0);
        let h = cross(d, e2);
        let a = dot(e1, h);
        if a.abs() < 1e-8 {
            return None;
        }
        let f = 1.0 / a;
        let s = sub(o, v0);
        let u = f * dot(s, h);
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = cross(s, e1);
        let v = f * dot(d, q);
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = f * dot(e2, q);
        (t > 0.0 && t < tmax).then_some(t)
    }

    fn trace_blas(bvh: &[u8], o: [f32; 3], d: [f32; 3], mut tmax: f32) -> Option<(u32, f32)> {
        let inv = [
            (1.0 / d[0]).clamp(-f32::MAX, f32::MAX),
            (1.0 / d[1]).clamp(-f32::MAX, f32::MAX),
            (1.0 / d[2]).clamp(-f32::MAX, f32::MAX),
        ];
        let root_off = rd32(bvh, 0) as usize;
        if root_off == 0 {
            return None;
        }
        let mut stack = vec![encode_node_id(root_off, NODE_BOX32)];
        let mut best: Option<(u32, f32)> = None;
        while let Some(node) = stack.pop() {
            if node == INVALID_NODE {
                continue;
            }
            let ty = node & 7;
            let off = ((node & !7) as usize) << 3;
            if ty == NODE_BOX32 {
                for i in 0..4 {
                    let cid = rd32(bvh, off + i * 4);
                    if cid == INVALID_NODE {
                        continue;
                    }
                    let bo = off + 16 + i * 24;
                    let mn = [rdf32(bvh, bo), rdf32(bvh, bo + 4), rdf32(bvh, bo + 8)];
                    let mx = [rdf32(bvh, bo + 12), rdf32(bvh, bo + 16), rdf32(bvh, bo + 20)];
                    if ray_aabb(o, inv, tmax, mn, mx).is_some() {
                        stack.push(cid);
                    }
                }
            } else if ty < 4 {
                let v0 = [rdf32(bvh, off), rdf32(bvh, off + 4), rdf32(bvh, off + 8)];
                let v1 = [rdf32(bvh, off + 12), rdf32(bvh, off + 16), rdf32(bvh, off + 20)];
                let v2 = [rdf32(bvh, off + 24), rdf32(bvh, off + 28), rdf32(bvh, off + 32)];
                let tid = rd32(bvh, off + 0x30);
                if let Some(t) = ray_tri(o, d, tmax, v0, v1, v2) {
                    tmax = t;
                    best = Some((tid, t));
                }
            }
        }
        best
    }

    #[test]
    fn blas_qbvh4_traces_triangles_accurately() {
        let mut tris = Vec::new();
        for i in 0..10u32 {
            let x = i as f32 * 3.0;
            tris.push(TriangleInput {
                v: [[x, -1.0, 5.0 + i as f32], [x + 2.0, -1.0, 5.0 + i as f32], [x, 1.0, 5.0 + i as f32]],
                geometry_index: 0,
                primitive_index: i,
                flags: GEOM_FLAG_OPAQUE,
            });
        }
        let blas = build_blas(&tris);
        assert_eq!(blas.leaf_count, 10);
        assert!(blas.internal_count >= 3);
        let pre = prebuild_info(AS_TYPE_BOTTOM_LEVEL, 10);
        assert!(blas.bytes.len() as u64 <= pre.result_data_max_size);

        let hit = trace_blas(&blas.bytes, [12.5, 0.0, 0.0], [0.0, 0.0, 1.0], 100.0).expect("hit tri 4");
        assert_eq!(hit.0, 4);
        assert!((hit.1 - 9.0).abs() < 1e-4);

        assert!(trace_blas(&blas.bytes, [100.0, 0.0, 0.0], [0.0, 0.0, 1.0], 100.0).is_none());
    }

    #[test]
    fn tlas_instance_inverse_transform_round_trips() {
        let otw = [
            2.0, 0.0, 0.0, 10.0,
            0.0, 2.0, 0.0, -4.0,
            0.0, 0.0, 2.0, 3.0,
        ];
        let wto = invert_3x4(&otw);
        let p = [1.5, -2.0, 4.0];
        let world = transform_point(&otw, p);
        let back = transform_point(&wto, world);
        for k in 0..3 {
            assert!((back[k] - p[k]).abs() < 1e-5);
        }

        let tlas = build_tlas(&[InstanceInput {
            transform: otw,
            instance_id: 42,
            mask: 0xff,
            sbt_offset: 3,
            flags: 1,
            blas_va: 0x10_0000_1000,
            blas_bounds: Aabb { min: [-1.0; 3], max: [1.0; 3] },
        }]);
        assert_eq!(tlas.leaf_count, 1);
        assert_eq!(tlas.bounds.min, [8.0, -6.0, 1.0]);
        assert_eq!(tlas.bounds.max, [12.0, -2.0, 5.0]);
        let pre = prebuild_info(AS_TYPE_TOP_LEVEL, 1);
        assert!(tlas.bytes.len() as u64 <= pre.result_data_max_size);
    }
}
