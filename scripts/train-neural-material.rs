use std::{f32::consts::TAU, io::Write};

const LAYERS: [(usize, usize, usize, usize); 3] = [(16, 32, 0, 512), (32, 32, 544, 1568), (32, 4, 1600, 2112)];
const COUNT: usize = 2128;

fn random(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state >> 8) as f32 / 16777216.0
}

fn sample(uv: [f32; 2]) -> ([f32; 32], [f32; 4]) {
    let [u, v] = uv.map(|x| x * TAU);
    let mut input = [0.0; 32];
    for (i, phase) in [u, v, u + v, u - v, 2.0 * u, 2.0 * v, 2.0 * u + v, u + 2.0 * v].iter().enumerate() {
        (input[2 * i], input[2 * i + 1]) = phase.sin_cos();
    }
    let wave = (u + 0.55 * v.sin() + 0.3 * (2.0 * v).sin()).sin();
    let band = 0.5 + 0.5 * wave;
    let vein = (1.0 - (wave - 0.15).abs() * 6.0).max(0.0).powi(2);
    let grain = 0.5 + 0.5 * (u + 2.0 * v).cos();
    let base = [0.025 + 0.06 * band, 0.10 + 0.26 * band, 0.14 + 0.29 * band];
    let gold = [0.72, 0.37, 0.09];
    (input, [base[0] + (gold[0] - base[0]) * vein, base[1] + (gold[1] - base[1]) * vein,
        base[2] + (gold[2] - base[2]) * vein, 0.22 + 0.18 * grain + 0.18 * vein])
}

fn main() -> std::io::Result<()> {
    let mut rng = 0x1729u32;
    let mut weights = [0.0f32; COUNT];
    let mut first = [0.0f32; COUNT];
    let mut second = [0.0f32; COUNT];
    for &(inputs, outputs, matrix, _) in &LAYERS {
        for w in &mut weights[matrix..matrix + inputs * outputs] { *w = (random(&mut rng) * 2.0 - 1.0) * (6.0 / inputs as f32).sqrt(); }
    }
    const STEPS: usize = 24000;
    const BATCH: usize = 64;
    for step in 1..=STEPS {
        let mut gradient = [0.0; COUNT];
        let mut loss = 0.0;
        for _ in 0..BATCH {
            let (input, target) = sample([random(&mut rng), random(&mut rng)]);
            let mut activations = [[0.0; 32]; 4];
            activations[0] = input;
            for (layer, &(inputs, outputs, matrix, bias)) in LAYERS.iter().enumerate() {
                for row in 0..outputs {
                    let mut value = weights[bias + row];
                    for col in 0..inputs { value += weights[matrix + row * inputs + col] * activations[layer][col]; }
                    activations[layer + 1][row] = if layer == 2 { value } else { value.max(0.0) };
                }
            }
            let mut delta = [0.0; 32];
            for i in 0..4 { let error = activations[3][i] - target[i]; delta[i] = error * (0.5 / BATCH as f32); loss += error * error; }
            for (layer, &(inputs, outputs, matrix, bias)) in LAYERS.iter().enumerate().rev() {
                let mut previous = [0.0; 32];
                for row in 0..outputs {
                    gradient[bias + row] += delta[row];
                    for col in 0..inputs {
                        let index = matrix + row * inputs + col;
                        gradient[index] += delta[row] * activations[layer][col];
                        previous[col] += weights[index] * delta[row];
                    }
                }
                for col in 0..inputs { if activations[layer][col] <= 0.0 { previous[col] = 0.0; } }
                delta = previous;
            }
        }
        let rate = 0.0015 * (1.0 - 0.9 * step as f32 / STEPS as f32);
        for i in 0..COUNT {
            first[i] = 0.9 * first[i] + 0.1 * gradient[i];
            second[i] = 0.999 * second[i] + 0.001 * gradient[i] * gradient[i];
            weights[i] -= rate * (first[i] / (1.0 - 0.9f32.powi(step as i32)))
                / ((second[i] / (1.0 - 0.999f32.powi(step as i32))).sqrt() + 1e-8);
        }
        if step % 2000 == 0 { println!("step {step}: RMSE {:.6}", (loss / (BATCH * 4) as f32).sqrt()); }
    }
    let destination = std::env::args().nth(1).unwrap_or_else(|| "content/neural_material.bin".into());
    let mut file = std::fs::File::create(&destination)?;
    file.write_all(b"ZNM1")?;
    file.write_all(&(COUNT as u32).to_le_bytes())?;
    for w in weights { file.write_all(&w.to_le_bytes())?; }
    println!("Wrote {destination}");
    Ok(())
}
