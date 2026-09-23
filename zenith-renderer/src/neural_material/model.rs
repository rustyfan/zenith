use anyhow::{Result, ensure};

pub const PARAMETER_COUNT: usize = 2128;
pub const LAYERS: [(usize, usize, usize, usize); 3] =
    [(16, 32, 0, 512), (32, 32, 544, 1568), (32, 16, 1600, 2112)];

pub fn features(uv: [f32; 2]) -> [f32; 16] {
    let [u, v] = uv.map(|x| x * std::f32::consts::TAU);
    let phases = [
        u,
        v,
        u + v,
        u - v,
        2.0 * u,
        2.0 * v,
        2.0 * u + v,
        u + 2.0 * v,
    ];
    std::array::from_fn(|i| {
        if i % 2 == 0 {
            phases[i / 2].sin()
        } else {
            phases[i / 2].cos()
        }
    })
}

pub fn reference(uv: [f32; 2]) -> [f32; 4] {
    let [u, v] = uv.map(|x| x * std::f32::consts::TAU);
    let wave = (u + 0.55 * v.sin() + 0.3 * (2.0 * v).sin()).sin();
    let band = 0.5 + 0.5 * wave;
    let vein = (1.0 - (wave - 0.15).abs() * 6.0).max(0.0).powi(2);
    let grain = 0.5 + 0.5 * (u + 2.0 * v).cos();
    let base = [0.025 + 0.06 * band, 0.10 + 0.26 * band, 0.14 + 0.29 * band];
    let gold = [0.72, 0.37, 0.09];
    [
        base[0] + (gold[0] - base[0]) * vein,
        base[1] + (gold[1] - base[1]) * vein,
        base[2] + (gold[2] - base[2]) * vein,
        0.22 + 0.18 * grain + 0.18 * vein,
    ]
}

pub fn evaluate(weights: &[f32], uv: [f32; 2]) -> [f32; 4] {
    let mut activation = [0.0; 32];
    activation[..16].copy_from_slice(&features(uv));
    for (layer, &(inputs, outputs, matrix, bias)) in LAYERS.iter().enumerate() {
        let mut next = [0.0; 32];
        for row in 0..outputs {
            let mut value = weights[bias + row];
            for column in 0..inputs {
                value += weights[matrix + row * inputs + column] * activation[column];
            }
            next[row] = if layer == 2 { value } else { value.max(0.0) };
        }
        activation = next;
    }
    std::array::from_fn(|i| activation[i].clamp(0.0, 1.0))
}

pub fn decode(bytes: &[u8]) -> Result<Vec<f32>> {
    ensure!(
        bytes.len() == 8 + PARAMETER_COUNT * 4 && &bytes[..4] == b"ZNM1",
        "invalid neural material model"
    );
    ensure!(
        u32::from_le_bytes(bytes[4..8].try_into()?) == PARAMETER_COUNT as u32,
        "invalid neural material parameter count"
    );
    let weights: Vec<_> = bytes[8..]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    ensure!(
        weights.iter().all(|w| w.is_finite() && w.abs() < 65504.0),
        "invalid neural material weights"
    );
    Ok(weights)
}
