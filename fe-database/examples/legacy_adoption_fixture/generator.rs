use serde_json::{Value, json};

pub const NAME: &str = "qa_legacy_asymmetric_v1";

/// Build the canonical two-material, embedded GLB for one preparation identity.
pub fn generate(instance: &str) -> Result<Vec<u8>, &'static str> {
    if instance.len() != 32 || !instance.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid_preparation_identity");
    }
    let mut binary = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut meshes = Vec::new();
    for (material, (lo, hi)) in [
        ([-0.8_f32, 0.0, -0.3], [0.8, 0.4, 0.3]),
        ([0.25, 0.4, -0.3], [0.8, 1.7, 0.3]),
    ]
    .into_iter()
    .enumerate()
    {
        let points = [
            [lo[0], lo[1], lo[2]],
            [hi[0], lo[1], lo[2]],
            [hi[0], hi[1], lo[2]],
            [lo[0], hi[1], lo[2]],
            [lo[0], lo[1], hi[2]],
            [hi[0], lo[1], hi[2]],
            [hi[0], hi[1], hi[2]],
            [lo[0], hi[1], hi[2]],
        ];
        let faces = [
            ([0, 3, 2, 1], [0.0_f32, 0.0, -1.0]),
            ([4, 5, 6, 7], [0.0, 0.0, 1.0]),
            ([0, 4, 7, 3], [-1.0, 0.0, 0.0]),
            ([1, 2, 6, 5], [1.0, 0.0, 0.0]),
            ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
            ([3, 7, 6, 2], [0.0, 1.0, 0.0]),
        ];
        let start = binary.len();
        for (face, _) in faces {
            for index in face {
                for n in points[index] {
                    binary.extend_from_slice(&n.to_le_bytes());
                }
            }
        }
        let positions = views.len();
        views.push(
            json!({"buffer":0,"byteOffset":start,"byteLength":binary.len()-start,"target":34962}),
        );
        let start = binary.len();
        for (_, normal) in faces {
            for _ in 0..4 {
                for n in normal {
                    binary.extend_from_slice(&n.to_le_bytes());
                }
            }
        }
        let normals = views.len();
        views.push(
            json!({"buffer":0,"byteOffset":start,"byteLength":binary.len()-start,"target":34962}),
        );
        let start = binary.len();
        for face in 0..6_u16 {
            for index in [0, 1, 2, 0, 2, 3] {
                binary.extend_from_slice(&(4 * face + index).to_le_bytes());
            }
        }
        let indices = views.len();
        views.push(
            json!({"buffer":0,"byteOffset":start,"byteLength":binary.len()-start,"target":34963}),
        );
        let base = accessors.len();
        accessors.push(json!({"bufferView":positions,"componentType":5126,"count":24,"type":"VEC3","min":lo,"max":hi}));
        accessors.push(json!({"bufferView":normals,"componentType":5126,"count":24,"type":"VEC3"}));
        accessors
            .push(json!({"bufferView":indices,"componentType":5123,"count":36,"type":"SCALAR"}));
        meshes.push(json!({"primitives":[{"attributes":{"POSITION":base,"NORMAL":base+1},"indices":base+2,"material":material,"mode":4}]}));
    }
    let document = json!({"asset":{"version":"2.0","generator":"fe-qa-legacy-v1","extras":{"preparation_id":instance}},
        "scene":0,"scenes":[{"nodes":[0,1]}],"nodes":[{"mesh":0},{"mesh":1}],"meshes":meshes,
        "materials":[{"pbrMetallicRoughness":{"baseColorFactor":[0.85,0.02,0.25,1.0],"metallicFactor":0.0,"roughnessFactor":0.7}},
                     {"pbrMetallicRoughness":{"baseColorFactor":[0.02,0.85,0.95,1.0],"metallicFactor":0.0,"roughnessFactor":0.7}}],
        "buffers":[{"byteLength":binary.len()}],"bufferViews":views,"accessors":accessors});
    let mut json = serde_json::to_vec(&document).map_err(|_| "fixture_encoding_failed")?;
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    while binary.len() % 4 != 0 {
        binary.push(0);
    }
    let length = 12 + 8 + json.len() + 8 + binary.len();
    if length > 1024 * 1024 {
        return Err("fixture_size_overflow");
    }
    let mut bytes = Vec::with_capacity(length);
    for word in [
        0x46546c67_u32,
        2,
        length as u32,
        json.len() as u32,
        0x4e4f534a,
    ] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend(json);
    for word in [binary.len() as u32, 0x004e4942] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend(binary);
    Ok(bytes)
}

/// Check the frozen canonical source, container and embedded dependency contract.
pub fn verify(bytes: &[u8], instance: &str) -> Result<Value, &'static str> {
    if bytes != generate(instance)? {
        return Err("fixture_source_changed");
    }
    let word = |at: usize| {
        u32::from_le_bytes(bytes[at..at + 4].try_into().expect("canonical header")) as usize
    };
    if word(0) != 0x46546c67 || word(4) != 2 || word(8) != bytes.len() || word(16) != 0x4e4f534a {
        return Err("invalid_glb_container");
    }
    let document: Value =
        serde_json::from_slice(&bytes[20..20 + word(12)]).map_err(|_| "invalid_glb_json")?;
    let tail = 20 + word(12);
    if word(tail + 4) != 0x004e4942 || tail + 8 + word(tail) != bytes.len() {
        return Err("invalid_glb_binary");
    }
    if document["buffers"][0].get("uri").is_some()
        || document.get("images").is_some()
        || document.get("extensionsRequired").is_some()
    {
        return Err("external_glb_dependency");
    }
    Ok(
        json!({"glb_version":2,"scenes":1,"meshes":2,"primitives":2,"triangles":24,"materials":2,
        "bounds_min":[-0.8_f32,0.0,-0.3],"bounds_max":[0.8_f32,1.7,0.3],"self_contained":true}),
    )
}
